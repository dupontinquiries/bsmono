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
    F12,
    Space,
    Comma,
    Period,
    B,
    M,
    H,
    T,
    G,
    D,
    L,
    I,
    U,
    /// The letter K. Not `K`: `key()` does `use K::*`, and a variant named
    /// like its enum makes that glob ambiguous.
    LetterK,
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

/// What the engine panel reports. Loading a multi-gigabyte checkpoint is
/// slow enough that "nothing happened yet" and "it failed" must be
/// distinguishable on screen.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineStatus {
    Idle,
    Loading(String),
    Ready(String),
    Failed(String),
}

impl EngineStatus {
    pub fn line(&self) -> String {
        match self {
            EngineStatus::Idle => "no model loaded".into(),
            EngineStatus::Loading(m) => format!("loading {m}…"),
            EngineStatus::Ready(d) => format!("ready — {d}"),
            EngineStatus::Failed(e) => format!("failed: {e}"),
        }
    }
}

/// Which provider the loop pane has selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    Ollama,
    Logscope,
    /// The in-process wgpu engine — the model resident in THIS process,
    /// no HTTP, no second process. See `aperture_engine`.
    Engine,
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
    /// Where you have jumped from and to (F12, the pickers), as edges —
    /// `aperture_core::nav`. Alt+Left / Alt+Right walk it.
    pub nav: aperture_core::nav::NavPath,
    /// A file switch refused because this buffer has unsaved edits. Asking
    /// for the SAME file again discards them; anything else re-arms.
    discard_armed: Option<PathBuf>,
    /// Buckets for the current workspace root, loaded on first Ctrl+K and
    /// again whenever the root changes. Saved on every change to a bucket
    /// (never per keystroke: a save is two git subprocesses).
    pub buckets: Option<aperture_core::bucket::Workspace>,
    /// Offer `strip_actions` to the on-screen keyboard while focused. Off by
    /// default so tests never talk to a keyboard running on the machine
    /// they run on; the binary turns it on.
    pub strip_enabled: bool,
    /// The focus state last told to the keyboard.
    strip_sent: Option<bool>,
    /// Finger gestures on the editor surface (touch_input.rs).
    pub touch: crate::touch_input::TouchInput,
    bucket_delete_armed: bool,
    pub lang: Language,
    pub syntax: Syntax,
    pub find: Option<FindState>,
    /// Voice capture/transcription state for `palette_ui::Mode::
    /// VoiceCommand`'s mic button -- feeds a real transcript into the
    /// SAME `run_voice_command` seam the mode's typed stand-in already
    /// uses, rather than a separate trigger. An earlier draft wired this
    /// through a standalone Ctrl+P handler in `view::frame` instead; that
    /// collided with the REAL, pre-existing Ctrl+P binding
    /// (`state.rs`'s `(P, true) => self.open_palette()`) and bypassed
    /// this mode entirely -- removed once found.
    #[cfg(feature = "voice")]
    pub voice_popup: uikit_egui::voice_popup::VoicePopup,
    /// The mic button drawn inside `Mode::VoiceCommand`'s row: click to
    /// start, right-click/hold to cancel.
    pub mic_indicator: uikit_egui::mic_indicator::MicIndicator,
    /// impakt-cam mode: the camera keeps the caret centered (dead zone +
    /// snap horizon live in `camera::Follow`). The view applies it only when
    /// the caret LINE changed, so wheel-browsing is free until you type.
    pub follow_mode: bool,
    /// Typewriter camera: the caret line rides the vertical CENTER of the
    /// viewport, film-style — the world moves, the caret holds. Strict
    /// sibling of follow_mode (no dead zone), with a heavier, slower
    /// convergence while on so the move reads as a camera dolly, not a
    /// scroll.
    pub typewriter_mode: bool,
    /// How the caret presents itself at rest, and — the part that matters
    /// off a wall socket — whether it asks for frames while doing it. See
    /// `camera::CaretPolicy`.
    pub caret: crate::camera::CaretPolicy,
    /// When the person last did anything. The caret's animation phase is
    /// anchored to this rather than to the clock, so it is fully lit the
    /// instant you type; it is also what decides when the caret has been
    /// left alone long enough to stop animating entirely.
    ///
    /// Deliberately "input", not "edit": scrolling and clicking are a
    /// person being present, and a caret that settles while someone is
    /// reading with the mouse is settling too eagerly.
    pub last_input_ms: u64,
    /// Text motion: when an edit changes the line count, lines below the
    /// edit slide from where they WERE into where they ARE. Measured in
    /// LINES (unit-agnostic, so detection is headless-testable); the view
    /// multiplies by line_h at paint. Snapped to ±d on the edit frame,
    /// decays to 0.
    pub line_shift: crate::camera::Smooth,
    pub line_shift_from: usize,
    last_motion_version: u64,
    last_len_lines: usize,
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
    pub logscope_cfg: aperture_complete::providers::LogscopeConfig,
    pub engine_cfg: aperture_engine::EngineConfig,
    /// GGUFs found on disk, for the model selector. Populated by
    /// `rescan_models()` rather than on every frame — a directory walk
    /// per frame is exactly the kind of thing that makes a UI feel bad.
    pub engine_models: Vec<aperture_engine::FoundModel>,
    /// The loaded model, shared: `build_provider` clones this handle
    /// instead of re-loading a checkpoint it must never re-load.
    pub engine_backend: Option<std::sync::Arc<dyn aperture_engine::Backend>>,
    /// What the pane reports about loading — a 15 GB load has to be
    /// visible, and a failure has to say what failed.
    pub engine_status: EngineStatus,
    /// Completions already answered, so caret motion and typing along a
    /// suggestion do not re-ask the model. See `aperture_complete::prefixcache`.
    pub prefix_cache: aperture_complete::prefixcache::PrefixCache,
    /// How much context a completion sees. Function scope by default —
    /// right more often than not, and cheap enough on a long file.
    pub scope: aperture_complete::scope::Scope,
    /// Modal mode, live only while typewriter mode is on — the two travel
    /// together because a centred film-style caret is a navigating
    /// posture and modal motion is what that posture wants.
    pub helix: aperture_core::helix::Mode,
    /// The mobile bottom strip: recents + pinned quick actions, and the
    /// hold gesture that opens the palette without a keyboard. Off until
    /// asked for — see `uikit_egui::cmdstrip`.
    pub cmd_strip: uikit_egui::cmdstrip::Strip,
    /// The multipass menu for the current reply: blocks beyond the ghost,
    /// each bound to a home-row key, plus the model's prose.
    ///
    /// Empty means "no menu showing", which is also the state ESC returns
    /// to. No separate mode flag: a flag and a menu that can disagree is
    /// how a UI gets stuck in a mode with nothing in it.
    pub menu: Vec<aperture_complete::multipass::Block>,
    /// Which key accepts which completion block.
    ///
    /// Editable, because `HOME_KEYS` is a sensible default and a wrong
    /// one on Dvorak, on AZERTY (no `;` on the home row at all), and on
    /// a split board with a different number of comfortable keys.
    pub complete_keys: aperture_complete::keymap::KeyMap,
    /// The block currently being typed out, a bit per Tab.
    ///
    /// `None` means no accept is in progress — there is no separate
    /// "accepting" flag, because a flag and a typeout that can disagree
    /// is how a UI gets stuck mid-suggestion.
    pub typeout: Option<aperture_complete::multipass::Typeout>,
    pub menu_prose: Vec<String>,
    /// Hold detection for the strip's palette button.
    pub cmd_hold: uikit_egui::cmdstrip::Hold,
    /// The last yank, for `p`. Editor-local rather than the system
    /// clipboard: `y`/`p` are motions in a modal grammar, and routing
    /// them through the OS would make them fight copy/paste.
    pub yank: String,
    /// How many lines `Scope::Line` includes. Configurable per the
    /// original request: a single line routinely misses the `if` guard
    /// or loop header sitting right above the caret.
    pub line_scope_lines: usize,
    /// The (prefix, lang) an in-flight request was built from, so its
    /// answer can be filed under the right key when it lands.
    pending_prefix: Option<(String, String)>,
    /// Line identities as of the last edit, for deciding which cached
    /// entries an edit actually invalidated.
    line_map: aperture_complete::prefixcache::LineMap,
    /// Swapped with `line_map` each edit so the Vec's capacity is reused
    /// instead of a fresh allocation per keystroke.
    line_map_scratch: aperture_complete::prefixcache::LineMap,
    pub command_line: String,

    /// Multi-cursor: the SECONDARY cursors. `editor.sel` stays the primary.
    ///
    /// Not `Cursors` wholesale, and not moved into `Editor`, on purpose. Every
    /// existing path — motion, find, ghost completion, IME, click, helix — is
    /// written against one `editor.sel`, and rewriting thirty call sites to be
    /// cursor-set-aware in one go is how a working editor stops working. With
    /// this vec empty (the overwhelmingly common case) not one of those paths
    /// changes behaviour or even branches differently; with it non-empty, the
    /// handful of paths that fan out consult `cursors()` and go through
    /// `aperture_core::multicursor`, which owns the offset arithmetic.
    ///
    /// The promotion into `Editor` stays available: `cursors()` is the only
    /// reader, so moving the storage later is a change to two functions rather
    /// than to the shell.
    pub extra_cursors: Vec<aperture_core::editor::Selection>,
    pub ghost_since: Option<u64>,
    pub palette: Option<crate::palette_ui::PaletteState>,
    pub theme_id: snapclone_theme::PaletteId,
    pub status: Option<(String, u64)>, // message, expires_at_ms
    /// `None` until a file recognized as having a configured server is
    /// opened AND spawning that server succeeds — optional and best-effort
    /// throughout, per `lsp_worker`'s own doc. Only Rust (`rust-analyzer`)
    /// is wired up; every other language simply never sets this.
    lsp: Option<crate::lsp_worker::LspWorker>,
    /// The root the running (or starting) server was given, and a root
    /// whose server failed to start — not retried this session.
    lsp_root: Option<String>,
    lsp_failed_root: Option<String>,
    lsp_uri: Option<String>,
    /// The document text as of the LAST successful sync (`didOpen` or
    /// `didChange`) — `notify_lsp_changed` diffs against this to compute
    /// an incremental-sync candidate, then updates it to the new text
    /// unconditionally (even when the worker ends up falling back to full
    /// sync) so the NEXT diff is still against what the server was told,
    /// not against some earlier state.
    lsp_last_synced_text: String,
    /// LSP's own per-document version counter — required by `didChange`,
    /// and deliberately NOT `editor.version` (that counts every internal
    /// mutation including ones the server was never told about, e.g. before
    /// this file had a worker at all; reusing it would let the two
    /// disagree about how many changes the server has actually seen).
    lsp_doc_version: i64,
    /// Bumped on every popup-refresh that fires an LSP request; an
    /// arriving [`lsp_worker::LspOutcome`] is applied only if this still
    /// matches — the same generation-gate `aperture_complete::engine`
    /// already uses so a slow answer can't land after the caret moved on.
    lsp_gen: u64,
    /// The word-start offset the current `lsp_gen` request was made for —
    /// `poll_lsp` needs this to open a FRESH popup when the local tier
    /// found nothing but LSP answers anyway (a std type invisible to the
    /// local single-file outline, for instance).
    lsp_pending_prefix_start: Option<usize>,
    /// Bumped on every go-to-definition request; an arriving
    /// `DefinitionOutcome` is applied only if this still matches — same
    /// generation-gate `lsp_gen` already is for completions.
    lsp_definition_gen: u64,
    /// Generation of the last Shift+F12 (callers) request.
    lsp_callers_gen: u64,
    /// Bumped on every hover request; an arriving `HoverOutcome` is
    /// applied only if this still matches.
    lsp_hover_gen: u64,
    /// The full flattened hover text for a real overlay panel, distinct
    /// from the one-line status message: the status headline exists so
    /// something useful shows up fast without blocking on scroll/dismiss
    /// interaction, but a real type signature plus doc comment is often
    /// several lines and deserves more than one line of status text.
    /// Cleared on Escape, on the next edit, or when a new hover request's
    /// answer supersedes it (never left showing stale text at a caret
    /// position the person has since moved away from).
    pub hover_overlay: Option<String>,
    /// Latest `publishDiagnostics` for the current file, empty when no LSP
    /// worker is running or the file is currently clean. Line numbers are
    /// LSP's own (zero-indexed), matching `git_changed`'s convention.
    pub lsp_diagnostics: Vec<aperture_lsp::Diagnostic>,
    /// Vocabulary buildup + per-speaker "again" history for voice commands
    /// — see `aperture_core::voice::VoiceSession`'s own doc for the honest
    /// scope (no microphone, no diarization; this is what happens once an
    /// utterance is already text and already attributed to someone).
    voice_session: aperture_core::voice::VoiceSession,
}

/// The strip as it starts: empty recents, a few pinned quick actions.
///
/// Pinned rather than left blank, because the first time someone turns
/// the strip on there ARE no recents — an empty bar beside a palette
/// button teaches that the feature does nothing, and they turn it back
/// off before it can learn anything.
///
/// Chosen for TOUCH specifically, which is what the strip is for. Save
/// and Find are the two things hardest to reach without a keyboard
/// (Ctrl+S, Ctrl+Shift+F), and "Complete now" is the one command a
/// person on a phone wants a button for rather than a debounce wait.
/// Everything else stays discoverable through the palette, which the
/// hold gesture reaches.
///
/// These fall off the visible strip as recents fill it, by design:
/// `Strip::entries` puts recents first, so the bar converges on what
/// this person actually uses and the defaults are only a starting
/// posture.
fn default_cmd_strip() -> uikit_egui::cmdstrip::Strip {
    use uikit_egui::cmdstrip::{Cmd, Strip};
    let mut s = Strip::new(5);
    for label in [
        "File: Save",
        "Find: In Project… (Ctrl+Shift+F)",
        "AI: Complete now",
    ] {
        // id == label: the palette has no separate command id, and
        // `run_strip_command` matches on the label for the same reason.
        s.pin(Cmd::new(label, label));
    }
    s
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
            nav: Default::default(),
            discard_armed: None,
            buckets: None,
            touch: Default::default(),
            strip_enabled: false,
            strip_sent: None,
            bucket_delete_armed: false,
            lang,
            syntax: Syntax::new(lang),
            find: None,
            #[cfg(feature = "voice")]
            voice_popup: Default::default(),
            mic_indicator: Default::default(),
            follow_mode: false,
            typewriter_mode: false,
            caret: crate::camera::CaretPolicy::default(),
            last_input_ms: 0,
            line_shift: crate::camera::Smooth::new(0.0, 14.0),
            line_shift_from: 0,
            last_motion_version: 0,
            last_len_lines: 1,
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
            logscope_cfg: aperture_complete::providers::LogscopeConfig::default(),
            engine_cfg: aperture_engine::EngineConfig::default(),
            engine_models: Vec::new(),
            engine_backend: None,
            engine_status: EngineStatus::Idle,
            prefix_cache: aperture_complete::prefixcache::PrefixCache::new(256),
            scope: aperture_complete::scope::Scope::default(),
            helix: aperture_core::helix::Mode::Normal,
            cmd_strip: default_cmd_strip(),
            menu: Vec::new(),
            typeout: None,
            complete_keys: aperture_complete::keymap::KeyMap::new(),
            menu_prose: Vec::new(),
            cmd_hold: uikit_egui::cmdstrip::Hold::new(),
            yank: String::new(),
            line_scope_lines: aperture_complete::scope::DEFAULT_LINE_COUNT,
            pending_prefix: None,
            line_map: aperture_complete::prefixcache::LineMap::new(),
            line_map_scratch: aperture_complete::prefixcache::LineMap::new(),
            command_line: String::new(),
            extra_cursors: Vec::new(),
            ghost_since: None,
            palette: None,
            theme_id: snapclone_theme::PaletteId::Night,
            status: None,
            lsp: None,
            lsp_root: None,
            lsp_failed_root: None,
            lsp_uri: None,
            lsp_last_synced_text: String::new(),
            lsp_doc_version: 1,
            lsp_gen: 0,
            lsp_pending_prefix_start: None,
            lsp_definition_gen: 0,
            lsp_callers_gen: 0,
            lsp_hover_gen: 0,
            hover_overlay: None,
            lsp_diagnostics: Vec::new(),
            voice_session: aperture_core::voice::VoiceSession::new(),
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
        // `open_file` isn't the only way a file gets loaded — the binary's
        // CLI-argument path (`aperture some_file.rs`) constructs AppState
        // with a path directly and never calls `open_file` at all. Found
        // by launching the real binary this way and watching the popup
        // never gain an LSP-sourced item: `start_lsp_for_current_file`
        // needs calling from both places, not just the in-app Open dialog.
        s.start_lsp_for_current_file(&text);
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
        engine.request(CompletionRequest { generation: self.note_generation, prefix: prompt, suffix: String::new(), lang: "text".into(), path: None });
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
        engine.request(CompletionRequest { generation: self.route_generation, prefix: prompt, suffix: String::new(), lang: "text".into(), path: None });
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
        self.discard_extra_cursors(); // replace_range isn't fan-out-aware
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
        self.discard_extra_cursors(); // a whole new Editor; old offsets mean nothing
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

    /// Re-scan the model directories. Called from the pane's button and
    /// once when the engine provider is first selected — not per frame.
    pub fn rescan_models(&mut self) {
        self.engine_models = aperture_engine::discover_models(&aperture_engine::default_model_dirs());
    }

    /// Load the configured model into this process.
    ///
    /// Synchronous and therefore BLOCKING, which is the honest shape for
    /// now: it is called from a button, not from the frame loop, and the
    /// status line says "loading" before it runs. Moving it to a worker
    /// thread is the right next step (the handle is already an Arc, so
    /// nothing else has to change) — but a background load that lies about
    /// being done would be worse than a window that is visibly busy.
    ///
    /// Without the `engine` feature this reports what is true — the build
    /// has no engine in it — rather than silently doing nothing.
    pub fn load_engine_model(&mut self) {
        #[cfg(feature = "engine")]
        {
            if self.engine_cfg.model_path.is_empty() {
                self.engine_status = EngineStatus::Failed("no model selected".into());
                return;
            }
            // DAEMON FIRST, then our own copy. A shared daemon holds one
            // set of weights for every app on the machine — 7.2 GiB and
            // ~86 seconds, paid once instead of per app. Loading our own
            // when one is already running is the situation the daemon
            // exists to avoid.
            //
            // The fallback is not an error path: an editor whose
            // completions stop because a background service is not
            // running would be worse than no daemon at all, since the
            // failure is invisible and looks like the editor's fault. So
            // `resolve` takes the first that CONSTRUCTS and carries on.
            use aperture_engine::{daemon, Reach};
            let cfg = self.engine_cfg.clone();
            let outcome = aperture_engine::resolve(
                &[Reach::Daemon, Reach::InProcess],
                || {
                    daemon::DaemonBackend::probe(daemon::socket_path())
                        .map(|b| std::sync::Arc::new(b) as std::sync::Arc<dyn aperture_engine::Backend>)
                },
                || {
                    aperture_engine::LocalEngine::load(cfg.clone())
                        .map(|e| e.handle())
                },
            );
            match outcome {
                Ok((e, reach)) => {
                    // Say WHICH, because "why is this fast today" and
                    // "why did my model selection do nothing" have the
                    // same answer and it is this.
                    let via = match reach {
                        Reach::Daemon => " via daemon",
                        Reach::InProcess => "",
                    };
                    self.engine_status = EngineStatus::Ready(format!("{}{via}", e.describe()));
                    self.engine_backend = Some(e.handle());
                }
                Err(e) => {
                    self.engine_backend = None;
                    self.engine_status = EngineStatus::Failed(e);
                }
            }
        }
        #[cfg(not(feature = "engine"))]
        {
            self.engine_status = EngineStatus::Failed(
                "built without the `engine` feature — rebuild with --features engine".into(),
            );
        }
    }

    pub fn build_provider(&self) -> Box<dyn Provider> {
        match self.provider_kind {
            ProviderKind::Ollama => Box::new(OllamaProvider::new(self.ollama_cfg.clone())),
            ProviderKind::Logscope => Box::new(aperture_complete::providers::LogscopeProvider::new(
                self.logscope_cfg.clone(),
            )),
            ProviderKind::Engine => match &self.engine_backend {
                // The shared handle, cloned — never a reload.
                Some(b) => Box::new(aperture_engine::EngineProvider::new(
                    self.engine_cfg.clone(),
                    aperture_engine::LocalEngine::shared(b.clone()),
                )),
                // Nothing loaded: fall back to the offline demo rather
                // than panicking or silently doing nothing. The pane says
                // why, right above the dropdown.
                None => Box::new(StaticProvider::new(&[])),
            },
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

    /// Change how much context completions see. Clears the ghost and the
    /// debounce: a suggestion computed at the old scope is an answer to a
    /// different question, and leaving it on screen would make the change
    /// look like it did nothing.
    pub fn set_scope(&mut self, scope: aperture_complete::scope::Scope, now_ms: u64) {
        self.scope = scope;
        self.ghost.dismiss();
        self.debounce.cancel();
        self.set_status(&format!("context scope: {} — {}", scope.label(), scope.hint()), now_ms);
    }

    pub fn toggle_typewriter(&mut self, now_ms: u64) {
        self.typewriter_mode = !self.typewriter_mode;
        // A camera dolly, not a scroll: while on, vertical convergence
        // slows from 12/s to 7/s so the centering move has WEIGHT. The
        // rate is restored on exit — browsing should stay snappy.
        self.cam.scroll_y.rate = if self.typewriter_mode { 7.0 } else { 12.0 };
        self.last_follow_line = usize::MAX; // re-center on the next frame
        let msg = if self.typewriter_mode {
            "typewriter camera: on — the line holds center, the world moves"
        } else {
            "typewriter camera: off"
        };
        self.set_status(msg, now_ms);
    }

    /// Per frame, from the view: detect line-count changes and arm the
    /// slide. In LINES, so this is pure state — the view converts to px.
    pub fn note_text_motion(&mut self) {
        if self.editor.version != self.last_motion_version {
            let now_lines = self.editor.buffer.len_lines();
            let d = now_lines as i64 - self.last_len_lines as i64;
            if d != 0 && d.unsigned_abs() <= 8 {
                // Lines below the edit start offset by the OLD delta and
                // decay home: an insert slides them down into place, a
                // delete slides them up. Big multi-line pastes (>8) skip
                // the effect — a page teleporting 40 lines reads as a
                // glitch, not motion.
                let caret_line = self.editor.caret_pos().line;
                self.line_shift_from = if d > 0 { caret_line } else { caret_line + 1 };
                self.line_shift.snap(-(d as f32));
                self.line_shift.target = 0.0;
            }
            self.last_motion_version = self.editor.version;
            self.last_len_lines = now_lines;
        }
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

    /// Where a completion accept should actually start REPLACING, distinct
    /// from `word_before_caret`'s own `start` — the bug two screenshots
    /// caught: selecting `StreamDeserializer` after typing `serde_json::`
    /// replaced the WHOLE `serde_json::` prefix with the bare label,
    /// producing `StreamDeserializer` instead of
    /// `serde_json::StreamDeserializer`.
    ///
    /// `word_before_caret` deliberately walks back through `::` too, so the
    /// local-symbol fuzzy matcher can score a query like `serde_json::Strea`
    /// against a full qualified name — that part is correct and unchanged.
    /// The bug is that `popup_accept` then used THAT SAME span as the
    /// replace-range, but LSP completion items at a qualified-path position
    /// are scoped to just the final segment (rust-analyzer's own
    /// `insertText` for a `mod::` completion is the bare item name, not the
    /// qualified path) — the client is expected to leave everything up to
    /// the last `::` untouched and only replace from there.
    ///
    /// Finds the char-index right after the LAST `::` in `word`, if any;
    /// `word_start` unchanged (falls back to matching `word_before_caret`'s
    /// own start) when there is no `::` at all, which is every ordinary
    /// non-path completion and must stay exactly as it worked before this
    /// fix.
    ///
    /// Works on `word`'s CHARS, not `str::rfind`'s byte offset — `word` is
    /// built by reversing a `Vec<char>` in `word_before_caret`, and
    /// `word_start`/the caret are char positions (walked via `char_at`),
    /// so a byte offset would only happen to agree with a char offset for
    /// pure-ASCII text.
    fn completion_replace_start(word_start: usize, word: &str) -> usize {
        let chars: Vec<char> = word.chars().collect();
        let mut last_colon_pair_end: Option<usize> = None;
        let mut i = 0;
        while i + 1 < chars.len() {
            if chars[i] == ':' && chars[i + 1] == ':' {
                last_colon_pair_end = Some(i + 2);
                i += 2; // don't let ":::" double-count the shared colon
            } else {
                i += 1;
            }
        }
        match last_colon_pair_end {
            Some(end) => word_start + end,
            None => word_start,
        }
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
        // The REPLACE boundary, distinct from `start` (which the fuzzy
        // matcher below still needs as the full word-with-`::` start for
        // scoring). See `completion_replace_start`'s own doc for the bug
        // this fixes: accepting a completion must not eat an already-typed
        // module-path prefix like `serde_json::`.
        let replace_start = Self::completion_replace_start(start, &word);
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
            // The LOCAL tier has nothing, but LSP might — a std type like
            // "String" is invisible to this file's own tree-sitter outline
            // by construction, not a matching failure to fix here. Don't
            // give up on the popup just because this tier came up empty;
            // request LSP and let `poll_lsp` open the popup itself if an
            // answer arrives before the person moves on.
            self.popup = None;
            self.request_lsp_completion(replace_start);
        } else {
            self.ghost.dismiss();
            self.popup = Some(Popup { items, sel: 0, prefix_start: replace_start });
            self.request_lsp_completion(replace_start);
        }
    }

    /// Ask the LSP worker (if one is running) for completions at the
    /// caret, tagged with a fresh generation so a slow answer that lands
    /// after the popup has moved on gets discarded by `poll_lsp` rather
    /// than silently appended to (or opening) whatever's showing by then.
    /// `prefix_start` is remembered alongside the generation so `poll_lsp`
    /// can construct a FRESH popup from an LSP-only answer, not just merge
    /// into one the local tier already opened.
    fn request_lsp_completion(&mut self, prefix_start: usize) {
        let (Some(lsp), Some(uri)) = (&self.lsp, &self.lsp_uri) else { return };
        self.lsp_gen += 1;
        self.lsp_pending_prefix_start = Some(prefix_start);
        let p = self.editor.caret_pos();
        // LSP's `position.character` is a UTF-16 code-unit offset, not a
        // char column (`caret_pos().col`) — they agree for ASCII and
        // ordinary BMP text (including CJK, Cyrillic, every script that
        // isn't specifically astral-plane) and diverge only for
        // codepoints needing a UTF-16 surrogate pair, which
        // `char_col_to_utf16` accounts for the same way `width.rs`
        // already does for on-screen visual columns — a different unit
        // again, computed by a different function, but the same shape of
        // fix for the same underlying reason (one char is not always one
        // unit of whatever the consumer actually counts in).
        let line_text = self.editor.buffer.line(p.line);
        let utf16_col = aperture_core::width::char_col_to_utf16(&line_text, p.col);
        lsp.request_completion(uri.clone(), p.line as u32, utf16_col, self.lsp_gen);
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
        // `handle_find` intercepts every event while the bar is open, before
        // `key()`'s multi-cursor arms ever run — `replace_current`/
        // `replace_all` mutate the buffer through `Editor` directly, with no
        // notion of `extra_cursors`. Collapsing here, once, is simpler and
        // safer than making the whole find/replace bar fan-out-aware.
        self.discard_extra_cursors();
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
        // Every event that reaches here is the person being present. One
        // site, at the top, before any early return — the alternative is
        // remembering to touch it in forty match arms, and the failure
        // mode of forgetting one is a caret that settles mid-session.
        self.last_input_ms = now_ms;
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
            Ev::Text(t) => {
                // MODAL FIRST, and only when typewriter mode is on. A
                // single character goes to the keymap; anything longer is
                // a paste or an IME commit and is always text — running a
                // multi-char string through a single-key table would
                // silently drop all but the first.
                let mut chars = t.chars();
                let one = match (chars.next(), chars.next()) {
                    (Some(c), None) => Some(c),
                    _ => None,
                };
                match one {
                    // THE MENU CLAIMS HOME-ROW KEYS WHILE IT IS UP, and
                    // only while it is up. That is the whole bargain: a
                    // letter builds a block when there is something to
                    // build, and types a letter the rest of the time.
                    // `accept_menu_key` returns false when nothing is
                    // bound to that key, so an unbound letter still
                    // reaches the buffer — the menu never swallows a
                    // keystroke it has no use for.
                    Some(c) if !self.menu.is_empty() && self.accept_menu_key(c, now_ms) => {}
                    Some(c) if self.helix_key(c, c.is_uppercase(), now_ms) => {}
                    _ => self.type_text(&t, now_ms),
                }
            }
            Ev::Paste(t) => {
                self.ghost.dismiss();
                self.debounce.cancel();
                self.discard_extra_cursors(); // paste isn't fan-out-aware
                self.editor.insert_text(&t, now_ms);
                self.after_edit(now_ms);
            }
            Ev::Copy => fx.clipboard = self.copy_selection(),
            Ev::Cut => {
                fx.clipboard = self.copy_selection();
                if !self.editor.sel.is_caret() {
                    self.discard_extra_cursors(); // cut isn't fan-out-aware
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

    // ── multi-cursor ──────────────────────────────────────────────────────
    //
    // `editor.sel` is always the primary; `extra_cursors` holds the rest.
    // These two are the ONLY place that boundary is crossed — everything
    // else either reads `editor.sel` (unaware there might be more) or calls
    // `fan_out_edit` (aware there might be). Assembling/disassembling a
    // `Cursors` here rather than storing one keeps every pre-existing
    // single-cursor call site correct by construction: they simply never see
    // the rest.

    /// Drop the secondary cursors WITHOUT trying to preserve them.
    ///
    /// Every command that mutates or replaces the buffer without going
    /// through `fan_out_ranges` must call this first. Char offsets in
    /// `extra_cursors` are only meaningful relative to the buffer they were
    /// computed against; undo, redo, paste, an AI edit landing, a revision
    /// preview, or opening a different file all change that buffer without
    /// telling `extra_cursors` anything shifted, so leaving them in place
    /// would silently fan a LATER edit out to stale, wrong, or (after undo
    /// shrinks the buffer) merely coincidentally-valid positions instead of
    /// failing loudly. Collapsing to the primary is the honest fallback: the
    /// gesture that triggered one of these commands almost never had
    /// multi-cursor in mind anyway.
    ///
    /// Found by running the binary, not by `cargo test`: Ctrl+Shift+L across
    /// three lines, type a replacement (fans out correctly), Ctrl+Z (buffer
    /// correctly restored, but `extra_cursors` were never told the buffer
    /// changed length — the very next keystroke would have fanned out to
    /// whatever those stale offsets now happened to land on).
    pub(crate) fn discard_extra_cursors(&mut self) {
        self.extra_cursors.clear();
    }

    /// The full cursor set, primary first.
    fn cursors(&self) -> aperture_core::multicursor::Cursors {
        if self.extra_cursors.is_empty() {
            aperture_core::multicursor::Cursors::single(self.editor.sel)
        } else {
            let mut sels = vec![self.editor.sel];
            sels.extend(self.extra_cursors.iter().copied());
            aperture_core::multicursor::Cursors::new(sels)
        }
    }

    /// Split a (possibly reordered, possibly merged) `Cursors` back into
    /// `editor.sel` + `extra_cursors`. `Cursors::new`'s primary-tracking is
    /// what makes this safe to call after a merge: the primary index still
    /// points at the right selection even though the vec was sorted.
    fn set_cursors(&mut self, c: aperture_core::multicursor::Cursors) {
        let all = c.all();
        self.editor.sel = c.primary();
        self.extra_cursors = all.iter().copied().filter(|s| *s != c.primary()).collect();
    }

    /// Apply arbitrary per-cursor `(start, end, text)` ranges as one undo
    /// step, then place a caret at each range's post-edit position.
    ///
    /// NOT `Cursors::after_insert`: that method assumes each edit's removed
    /// span is exactly the corresponding *selection* (`s.max() - s.min()`),
    /// which is true for typing-over-a-selection but false for backspace,
    /// where a caret (an empty selection) still removes one character. Ranges
    /// here are already char index, so the same ascending-shift arithmetic
    /// applies — it just has to read `removed` from the range instead of
    /// from the original `Selection`.
    ///
    /// `coalesce` is threaded straight to `Editor`'s coalescing choice
    /// between `replace_many` (a fresh undo step, always) and
    /// `replace_many_coalescing` (may merge into the previous keystroke's
    /// group) — the same fork `insert_text`/`backspace` make per character.
    fn fan_out_ranges(&mut self, ranges: Vec<(usize, usize, String)>, coalesce: bool, now_ms: u64) {
        let new_carets = Self::carets_after_ranges(&ranges);
        if coalesce {
            self.editor.replace_many_coalescing(ranges, true, now_ms);
        } else {
            self.editor.replace_many(ranges, now_ms);
        }
        self.set_cursors(aperture_core::multicursor::Cursors::new(new_carets));
    }

    /// Where each cursor ends up after `ranges` is applied — computed
    /// up front, against the ORIGINAL offsets, the same way `replace_many`
    /// itself does internally. Kept separate from `Editor` (which has no
    /// notion of "cursor") rather than derived from `editor.sel` afterward,
    /// since a fan-out edit leaves only ONE caret in `editor.sel` and the
    /// rest have to be reconstructed from the ranges regardless.
    fn carets_after_ranges(ranges: &[(usize, usize, String)]) -> Vec<Selection> {
        let mut sorted: Vec<&(usize, usize, String)> = ranges.iter().collect();
        sorted.sort_by_key(|r| r.0);
        let mut shift: isize = 0;
        let mut new_carets = Vec::with_capacity(sorted.len());
        for (start, end, text) in sorted {
            let grow = text.chars().count() as isize;
            let removed = (*end as isize) - (*start as isize);
            let at = (*start as isize + shift + grow).max(0) as usize;
            new_carets.push(Selection::caret(at));
            shift += grow - removed;
        }
        new_carets
    }

    /// Tab at every cursor: each pads to ITS OWN next tab stop, not a shared
    /// literal `\t` — `Editor::insert_tab` never inserts a real tab
    /// character (the whole file is space-indented; `TAB_WIDTH` is the stop
    /// distance, not a byte to insert), and two cursors at different columns
    /// legitimately need a different number of spaces.
    fn fan_out_insert_tab(&mut self, now_ms: u64) {
        let c = self.cursors();
        let ranges: Vec<(usize, usize, String)> = c.all().iter().map(|s| {
            let col = self.editor.buffer.char_to_pos(s.min()).col;
            let n = aperture_core::editor::TAB_WIDTH - (col % aperture_core::editor::TAB_WIDTH);
            (s.min(), s.max(), " ".repeat(n))
        }).collect();
        self.fan_out_ranges(ranges, false, now_ms);
    }

    fn fan_out_insert(&mut self, text: &str, now_ms: u64) {
        let c = self.cursors();
        let ranges: Vec<(usize, usize, String)> =
            c.all().iter().map(|s| (s.min(), s.max(), text.to_string())).collect();
        // Same rule as `Editor::insert_text`: only a single non-newline
        // character coalesces with the run before it.
        let coalesce = text.chars().count() == 1 && text != "\n";
        self.fan_out_ranges(ranges, coalesce, now_ms);
    }

    /// Backspace at every cursor: each becomes a one-char-back deletion (or,
    /// if it already has a selection, that selection).
    fn fan_out_backspace(&mut self, now_ms: u64) {
        let c = self.cursors();
        // Coalescing only when EVERY cursor is a plain caret deleting a
        // single non-newline character — the same condition `Editor::
        // backspace` checks for its one cursor. A cursor with a live
        // selection makes this a "delete selection" gesture instead of a
        // keystroke, and mixing the two into one fan-out group would let an
        // unrelated multi-char deletion silently absorb into a typing run.
        let coalesce = c.all().iter().all(|s| {
            s.is_caret() && self.editor.buffer.char_at(s.head.saturating_sub(1)) != Some('\n')
        });
        let ranges: Vec<(usize, usize, String)> = c.all().iter().map(|s| {
            if s.is_caret() {
                let end = s.head;
                let start = self.editor.buffer.clamp(end.saturating_sub(1));
                (start, end, String::new())
            } else {
                (s.min(), s.max(), String::new())
            }
        }).collect();
        self.fan_out_ranges(ranges, coalesce, now_ms);
    }

    fn fan_out_delete_forward(&mut self, now_ms: u64) {
        let c = self.cursors();
        let ranges: Vec<(usize, usize, String)> = c.all().iter().map(|s| {
            if s.is_caret() {
                let start = s.head;
                let end = self.editor.buffer.clamp(start + 1);
                (start, end, String::new())
            } else {
                (s.min(), s.max(), String::new())
            }
        }).collect();
        // `Editor::delete_forward` never coalesces (`false`, unconditionally)
        // — matched here rather than given its own rule.
        self.fan_out_ranges(ranges, false, now_ms);
    }

    fn type_text(&mut self, t: &str, now_ms: u64) {
        self.preedit.clear(); // a real insertion always ends any pending composition
        if !self.extra_cursors.is_empty() {
            // Ghost completion, auto-pair and the symbol popup are all keyed
            // to ONE caret's context — none of the three has a fan-out story,
            // and guessing one is worse than not having it. Multi-cursor
            // typing is plain insertion, at every cursor, nothing speculative
            // shown at any of them.
            self.ghost.dismiss();
            self.debounce.cancel();
            self.fan_out_insert(t, now_ms);
            self.after_edit(now_ms);
            return;
        }
        if self.try_auto_pair(t, now_ms) {
            self.popup = None;
            return;
        }
        let consumed = self.ghost.on_text_inserted(t);
        // TYPING IS ALWAYS ALLOWED, mid-accept included — the "go back to
        // typing at any time" half of the brief.
        //
        // The typeout is dropped on ANY keystroke, including one the
        // ghost absorbed. That case looks like it should be survivable —
        // the person typed exactly what the suggestion said next — but
        // `on_text_inserted` shrinks the GHOST while the typeout's offset
        // into its own copy does not move, so the two would disagree by
        // one word and the next Tab would re-insert text already typed.
        //
        // Dropping costs nothing: the next Tab rebuilds a typeout from
        // whatever the ghost now says, which is exactly the remaining
        // suggestion. One source of truth beats two that need syncing.
        self.typeout = None;
        self.editor.insert_text(t, now_ms);
        // `after_edit`, not the three lines it inlines to be (dirty=true;
        // syntax.absorb; follow_caret=true) — a REAL, pre-existing bug
        // found while writing a test for hover_overlay's edit-dismissal
        // behavior: this branch had drifted out of sync with after_edit as
        // it gained more responsibilities (notify_lsp_changed,
        // invalidate_cache_for_edit, clearing hover_overlay) that ORDINARY
        // TYPING — the single most common edit there is — silently never
        // ran. Concretely: `didChange` was NEVER sent to the LSP worker
        // for a plain keystroke, only for backspace/delete/enter/tab and
        // the multi-cursor fan-out path, which is why every LSP e2e test
        // so far happened to still pass — completion at a syntactically
        // valid position inside an unchanged-since-didOpen function body
        // returns the same in-scope candidates whether or not the typed
        // prefix was ever communicated, so the tests were unknowingly
        // exercising "completion works when nothing was typed into the
        // synced copy" rather than proving sync itself worked for typing.
        self.after_edit(now_ms);
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
        self.invalidate_cache_for_edit();
        self.notify_lsp_changed();
        self.hover_overlay = None; // stale the instant the buffer under it changes
        let _ = now_ms;
    }

    /// `didChange` on every edit, unconditionally — no debounce, unlike the
    /// AI completion path. A stale server-side document view would answer
    /// completions against text that no longer exists, which is a worse
    /// failure than one extra IPC message per keystroke costs; the worker
    /// thread absorbs the cost off the UI thread regardless (see
    /// `lsp_worker`'s own doc for why this is safe to call often).
    fn notify_lsp_changed(&mut self) {
        let (Some(lsp), Some(uri)) = (&self.lsp, &self.lsp_uri) else { return };
        self.lsp_doc_version += 1;
        let new_text = self.editor.buffer.to_string();
        // Computed against the LAST text the server was actually told
        // about, not against some other snapshot — the whole point of
        // incremental sync is describing what changed since THAT point.
        let edit = aperture_core::textdiff::diff(&self.lsp_last_synced_text, &new_text).map(|e| {
            // LSP positions are UTF-16 code units; textdiff's Pos is char-
            // indexed (see width.rs's own doc on why these differ, and
            // request_lsp_completion for the same conversion applied to a
            // single position rather than a range). The line text has to
            // come from OLD_TEXT's own lines, not the current buffer's —
            // the edit's start/end are positions WITHIN the old text, and
            // the current buffer may have since diverged further if
            // several edits landed before this sync point (unlikely given
            // this runs from after_edit's own choke point, but the
            // correctness argument shouldn't depend on that).
            let old_lines: Vec<&str> = self.lsp_last_synced_text.split('\n').collect();
            let line_text = |line: usize| old_lines.get(line).copied().unwrap_or("");
            let start_col = aperture_core::width::char_col_to_utf16(line_text(e.start.line), e.start.col);
            let end_col = aperture_core::width::char_col_to_utf16(line_text(e.end.line), e.end.col);
            (e.start.line as u32, start_col, e.end.line as u32, end_col, e.text)
        });
        self.lsp_last_synced_text = new_text.clone();
        lsp.change(uri.clone(), new_text, edit, self.lsp_doc_version);
    }

    /// Apply an arrived LSP completion outcome to the popup, if it's still
    /// for the request the popup is actually waiting on. Called once per
    /// frame from the view's tick, same shape as `poll_completions`.
    pub fn poll_lsp(&mut self, now_ms: u64) {
        // The handshake runs off the UI thread (lsp_worker::spawn); a server
        // that failed it is dropped here, quietly — no rust-analyzer is the
        // common case and the local tier still works.
        if let Some(Err(_)) = self.lsp.as_ref().and_then(|l| l.poll_ready()) {
            self.lsp = None;
            self.lsp_failed_root = self.lsp_root.take();
            return;
        }
        let Some(lsp) = &self.lsp else { return };
        // Drained into a local Vec, not processed inline: `jump_to_lsp_
        // definition` needs `&mut self` (it can call `open_file`, which
        // reassigns `self.lsp` itself), so it can't run while `lsp` — a
        // borrow of `self.lsp` — is still alive. Collecting first, then
        // dropping `lsp` before the second loop, is the whole fix.
        let mut definition_outcomes = Vec::new();
        while let Some(outcome) = lsp.poll_definition() {
            definition_outcomes.push(outcome);
        }
        let mut callers_outcomes = Vec::new();
        while let Some(outcome) = lsp.poll_callers() {
            callers_outcomes.push(outcome);
        }
        let mut hover_outcomes = Vec::new();
        while let Some(outcome) = lsp.poll_hover() {
            hover_outcomes.push(outcome);
        }
        while let Some(diags) = lsp.poll_diagnostics() {
            self.lsp_diagnostics = diags;
        }
        while let Some(outcome) = lsp.poll() {
            if outcome.generation != self.lsp_gen {
                continue; // superseded by a later request; the caret moved on
            }
            match &mut self.popup {
                Some(popup) => {
                    let existing: std::collections::HashSet<String> =
                        popup.items.iter().map(|i| i.label.clone()).collect();
                    for item in outcome.items {
                        if existing.contains(&item.label) {
                            continue; // already offered by the local-symbol tier
                        }
                        popup.items.push(PopupItem {
                            label: item.label,
                            insert: item.insert_text,
                            detail: item.detail.unwrap_or_default(),
                        });
                    }
                }
                None => {
                    // The local tier had nothing (e.g. a std type this
                    // file never defines) — LSP answering anyway is reason
                    // enough to open the popup fresh, provided the person
                    // hasn't since moved the caret elsewhere.
                    let Some(prefix_start) = self.lsp_pending_prefix_start else { continue };
                    if outcome.items.is_empty() {
                        continue;
                    }
                    let items: Vec<PopupItem> = outcome
                        .items
                        .into_iter()
                        .map(|item| PopupItem {
                            label: item.label,
                            insert: item.insert_text,
                            detail: item.detail.unwrap_or_default(),
                        })
                        .collect();
                    self.popup = Some(Popup { items, sel: 0, prefix_start });
                }
            }
        }
        // `lsp` (borrowed from `self.lsp`) is no longer used past this
        // point, so `&mut self` calls below are fine — this is why
        // `definition_outcomes` was collected rather than processed above.
        for outcome in definition_outcomes {
            if outcome.generation != self.lsp_definition_gen {
                continue; // superseded by a later lookup; the caret moved on
            }
            match outcome.locations.first().cloned() {
                Some(loc) => self.jump_to_lsp_definition(&loc, now_ms),
                None => self.set_status("go to definition: nothing found", now_ms),
            }
        }
        for outcome in callers_outcomes {
            if outcome.generation != self.lsp_callers_gen {
                continue; // superseded
            }
            self.show_callers(outcome, now_ms);
        }
        for outcome in hover_outcomes {
            if outcome.generation != self.lsp_hover_gen {
                continue; // superseded by a later lookup; the caret moved on
            }
            match outcome.hover {
                Some(h) => {
                    let headline = Self::hover_headline(&h.contents);
                    self.set_status(&headline, now_ms);
                    self.hover_overlay = Some(h.contents);
                }
                None => {
                    self.set_status("hover: nothing found", now_ms);
                    self.hover_overlay = None;
                }
            }
        }
    }

    /// Pick the useful line out of a flattened hover text for a single-line
    /// status message. NOT just the first line: rust-analyzer's own hover
    /// convention for a function/type commonly puts a bare crate-name
    /// annotation line first (found by running this exact scenario — a
    /// hover for a same-crate function came back with the status bar
    /// showing the crate's own package name, not the signature, because
    /// the naive "first line" version of this code took it literally).
    /// Prefers the first line that actually looks like Rust code (a
    /// keyword or `->` in it); falls back to the first non-empty line if
    /// nothing matches, so a server with a different convention still
    /// shows SOMETHING rather than an empty status.
    fn hover_headline(contents: &str) -> String {
        const SIGNATURE_MARKERS: &[&str] =
            &["fn ", "struct ", "enum ", "trait ", "impl ", "type ", "const ", "static ", "let ", "->"];
        contents
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && *l != "```rust" && *l != "```")
            .find(|l| SIGNATURE_MARKERS.iter().any(|m| l.contains(m)))
            .or_else(|| contents.lines().map(str::trim).find(|l| !l.is_empty()))
            .unwrap_or("")
            .to_string()
    }

    /// Retire cached completions whose context genuinely left the buffer.
    ///
    /// Keyed on line CONTENT, not line numbers: deleting a line renumbers
    /// everything below it, and an entry computed from an untouched region
    /// is still correct however far it moved. Only text that is now absent
    /// from the buffer entirely invalidates anything.
    fn invalidate_cache_for_edit(&mut self) {
        // Rebuild in place: `LineMap` keeps its Vec's capacity across
        // clears, so a steady edit loop on one file stops allocating for
        // the hash side entirely after the first pass. The per-line
        // `into_owned()` remains — `Buffer::line` returns a Cow and a
        // rope chunk boundary can force an owned String — but it is one
        // short-lived allocation rather than the two (Vec<String> plus a
        // per-distinct-line map entry) this used to do every keystroke.
        let mut next = std::mem::take(&mut self.line_map_scratch);
        next.rebuild_from(self.editor.buffer.len_lines(), |i| self.editor.buffer.line(i));
        if !self.line_map.is_empty() {
            let gone = self.line_map.diff(&next);
            self.prefix_cache.invalidate_line_hashes(&gone);
        }
        // Swap rather than assign: the OLD map's allocation becomes next
        // frame's scratch instead of being dropped and re-made.
        self.line_map_scratch = std::mem::replace(&mut self.line_map, next);
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
        // A key that edits or moves ends any composition on screen. The
        // preedit was cleared only by an empty Preedit or a TEXT insertion,
        // so Enter / Tab / Backspace / arrows left the last composing text
        // painted at the NEW caret -- the Fedora (IBus) report: `"strings"`,
        // Enter, and `ngs"` drawn on the new indented line, not in the
        // buffer. A composition that really continues sends its preedit
        // again and is shown again.
        if !self.preedit.is_empty() && !matches!(key, Escape) {
            self.preedit.clear();
        }
        // Popup first: an explicit list outranks the speculative ghost.
        if self.popup_key(key, shift, now_ms) {
            return;
        }
        // Ghost interception next: Tab / Ctrl+→ / Esc while shown.
        if self.ghost.is_shown() {
            match (key, ctrl) {
                (Tab, false) => {
                    // TAB TAKES A BIT, not the whole block.
                    //
                    // Accepting everything was the gamble: a 12-line
                    // suggestion is right for 9 lines and wrong for 3,
                    // and taking it whole means deleting 3 afterwards —
                    // the manual cleanup this work exists to remove. A
                    // bit at a time means the wrong part is never typed.
                    //
                    // The typeout is created on the FIRST Tab against the
                    // current ghost, so nothing changes for someone who
                    // presses Tab once and keeps going: they get the
                    // first token instead of the whole block, and Tab
                    // again continues it.
                    if self.typeout.is_none() {
                        if let Some(text) = self.ghost.text() {
                            self.typeout =
                                Some(aperture_complete::multipass::Typeout::new(text));
                        }
                    }
                    // The borrow on `self.typeout` is CLOSED before any
                    // `self.` call: `after_edit` takes `&mut self`, and
                    // holding the typeout across it is E0499. Taking the
                    // bit and the done-flag out in one small scope is
                    // the fix, and it is why this reads as three steps
                    // rather than one.
                    let taken = self.typeout.as_mut().and_then(|t| {
                        t.next_bit().map(|b| b.to_string()).map(|b| (b, t.done()))
                    });
                    match taken {
                        Some((bit, done)) => {
                            self.editor.insert_text(&bit, now_ms);
                            self.after_edit(now_ms);
                            self.debounce.cancel();
                            // The ghost has to shrink by exactly what was
                            // typed, or it would redraw the accepted text
                            // ahead of the caret as if it were still
                            // pending.
                            self.ghost.consume(&bit);
                            if done {
                                self.typeout = None;
                            }
                        }
                        // Exhausted: drop it so the next Tab rebuilds
                        // from whatever the ghost now holds.
                        None => self.typeout = None,
                    }
                    return;
                }
                // The whole block in one press, for when it is obviously
                // right and picking through it is just slower. Ctrl-Tab
                // rather than a second Tab: a double-press that means
                // "all of it" would make an accidental repeat unrecoverable.
                (Tab, true) => {
                    let rest = self
                        .typeout
                        .as_mut()
                        .and_then(|t| t.rest_of_it().map(str::to_string))
                        .or_else(|| self.ghost.accept_all());
                    if let Some(text) = rest {
                        self.editor.insert_text(&text, now_ms);
                        self.after_edit(now_ms);
                        self.debounce.cancel();
                    }
                    self.typeout = None;
                    self.ghost.dismiss();
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
            (LetterK, true) => self.open_buckets(now_ms),
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
            (F12, false) if shift => self.find_callers(now_ms),
            (F12, false) => self.go_to_definition(now_ms),
            (I, true) => self.request_hover(now_ms),
            (Space, true) if shift => self.trigger_completion_now(now_ms),
            (Space, true) => self.refresh_popup(1),
            // ABOVE every other Left/Right arm: the match is on (key, ctrl)
            // and a guard-less arm first would take Alt+Left with no warning.
            (Left, _) if alt => self.nav_go(true, now_ms),
            (Right, _) if alt => self.nav_go(false, now_ms),
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
            // ORDER MATTERS and the compiler will not warn: a bare
            // `(D, true)` above this would swallow Ctrl+Shift+D, and the
            // symptom is a second cursor going the wrong way, not an error.
            (D, true) if shift => self.add_prev_cursor(now_ms),
            (D, true) => self.add_next_cursor(now_ms),
            (U, true) => self.pop_cursor(now_ms),
            (L, true) if shift => self.select_all_cursor_occurrences(now_ms),
            (Up, true) if alt => self.add_column_cursor(-1, now_ms),
            (Down, true) if alt => self.add_column_cursor(1, now_ms),
            (Backspace, _) => {
                self.ghost.dismiss();
                if !self.extra_cursors.is_empty() {
                    self.fan_out_backspace(now_ms);
                    self.after_edit(now_ms);
                } else if !self.backspace_pair(now_ms) {
                    self.editor.backspace(now_ms);
                    self.after_edit(now_ms);
                }
                self.arm_completion(now_ms);
            }
            (Delete, _) => {
                self.ghost.dismiss();
                if !self.extra_cursors.is_empty() {
                    self.fan_out_delete_forward(now_ms);
                } else {
                    self.editor.delete_forward(now_ms);
                }
                self.after_edit(now_ms);
            }
            (Enter, _) => {
                self.ghost.dismiss();
                if !self.extra_cursors.is_empty() {
                    self.fan_out_insert("\n", now_ms);
                } else {
                    self.editor.newline_smart(now_ms);
                }
                self.after_edit(now_ms);
                self.arm_completion(now_ms);
            }
            (Tab, false) => {
                if !self.extra_cursors.is_empty() {
                    self.fan_out_insert_tab(now_ms);
                } else {
                    self.editor.insert_tab(now_ms);
                }
                self.after_edit(now_ms);
            }
            (Escape, _) => {
                // ESC IS ALWAYS THE WAY BACK TO TYPING. Clearing the menu
                // first so one press does the obvious thing: the menu is
                // the loudest thing on screen when it is up, so it is
                // what a person means by "get out of this".
                // A half-typed block stops where it is. What was already
                // inserted stays — it is real text the person watched
                // arrive — and the rest is simply never typed, which is
                // the whole point of a granular accept.
                self.typeout = None;
                self.dismiss_menu();
                self.ghost.dismiss();
                self.debounce.cancel();
                self.hover_overlay = None;
                // Multiple cursors collapse to the primary — the one the
                // user had before Ctrl+D/Ctrl+Shift+L added the rest — same
                // as every other editor's Escape, and one keystroke closer
                // to a known state, which is what Escape is for.
                if !self.extra_cursors.is_empty() {
                    self.extra_cursors.clear();
                } else if self.typewriter_mode {
                    self.helix = aperture_core::helix::Mode::Normal;
                }
            }
            _ => {}
        }
        let _ = fx;
    }

    /// Ctrl+D: select the word under the caret, or add the next occurrence
    /// of what's already selected. `None` means the gesture had nothing to
    /// do (no word under a lone caret, or no further occurrence) — reported
    /// rather than silently ignored, the same as the palette's "no matches".
    fn add_next_cursor(&mut self, now_ms: u64) {
        let c = self.cursors();
        match aperture_core::multicursor::add_next_occurrence(&self.editor.buffer, &c) {
            Some(next) => {
                self.set_cursors(next);
                self.follow_caret = true;
                self.ghost.dismiss();
                self.popup = None; // its accept path isn't fan-out-aware either
            }
            None => self.set_status("Ctrl+D: nothing more to select", now_ms),
        }
    }

    /// Ctrl+Shift+D: the mirror of Ctrl+D, walking backward through the
    /// document. Kess's note was that Ctrl+D had no complement; his
    /// suggested binding (B) is taken by `branch_from_here`, so this went
    /// on Shift+D, where it sits next to the gesture it mirrors.
    fn add_prev_cursor(&mut self, now_ms: u64) {
        let c = self.cursors();
        match aperture_core::multicursor::add_prev_occurrence(&self.editor.buffer, &c) {
            Some(next) => {
                self.set_cursors(next);
                self.follow_caret = true;
                self.ghost.dismiss();
                self.popup = None;
            }
            None => self.set_status("Ctrl+Shift+D: nothing earlier to select", now_ms),
        }
    }

    /// Ctrl+U: drop the cursor the last Ctrl+D or Ctrl+Shift+D added.
    ///
    /// The other half of Kess's note, which reads two ways — "add the
    /// previous occurrence" and "undo the last cursor" are different
    /// operations and both were missing. Overshooting a run of Ctrl+D is
    /// the common case and Escape (collapse to one) is far too big a hammer
    /// for it.
    fn pop_cursor(&mut self, now_ms: u64) {
        let c = self.cursors();
        match aperture_core::multicursor::pop_cursor(&c) {
            Some(next) => {
                self.set_cursors(next);
                self.follow_caret = true;
                self.ghost.dismiss();
                self.popup = None;
            }
            None => self.set_status("Ctrl+U: only one cursor", now_ms),
        }
    }

    /// Ctrl+Shift+L: a cursor on every occurrence of the current word or
    /// selection.
    fn select_all_cursor_occurrences(&mut self, now_ms: u64) {
        let c = self.cursors();
        match aperture_core::multicursor::select_all_occurrences(&self.editor.buffer, &c) {
            Some(all) => {
                self.set_cursors(all);
                self.ghost.dismiss();
                self.popup = None;
            }
            None => self.set_status("Ctrl+Shift+L: no other occurrences", now_ms),
        }
    }

    /// Ctrl+Alt+Up / Ctrl+Alt+Down: add a caret one line above/below the
    /// outermost existing cursor in that direction — holding the key grows
    /// a column instead of fighting over the same gap each press.
    fn add_column_cursor(&mut self, dy: isize, now_ms: u64) {
        let c = self.cursors();
        match aperture_core::multicursor::add_vertical(&self.editor.buffer, &c, dy) {
            Some(next) => {
                self.set_cursors(next);
                self.follow_caret = true;
                self.ghost.dismiss();
                self.popup = None;
            }
            None => {
                let dir = if dy < 0 { "up" } else { "down" };
                self.set_status(&format!("no line {dir} of the file to add a cursor on"), now_ms);
            }
        }
    }

    fn with_dismiss(&mut self, f: impl FnOnce(&mut Self), _now_ms: u64) {
        self.ghost.dismiss();
        self.debounce.cancel();
        self.discard_extra_cursors(); // undo/redo change the buffer without fanning out
        f(self);
        self.dirty = true;
        self.syntax.absorb(&mut self.editor.buffer);
        self.follow_caret = true;
    }

    /// Run a motion against every cursor, not just the primary.
    ///
    /// `f` only ever touches `editor.sel` (that's the whole existing motion
    /// API), so fanning it out is: run it once per cursor with THAT cursor
    /// loaded into `editor.sel`, and collect where it ends up. No new motion
    /// code, no per-cursor duplication of `move_word`/`move_vertical`/etc —
    /// the single-cursor implementations are reused unmodified.
    ///
    /// `Fn`, not `FnOnce`: every existing call site is a closure over nothing
    /// but a couple of `Copy` primitives, so this costs those sites nothing.
    fn moved(&mut self, f: impl Fn(&mut Self, bool), shift: bool) {
        self.ghost.dismiss();
        self.debounce.cancel();
        if self.extra_cursors.is_empty() {
            f(self, shift);
        } else {
            let extras = std::mem::take(&mut self.extra_cursors);
            let mut new_sels = Vec::with_capacity(extras.len() + 1);
            f(self, shift);
            new_sels.push(self.editor.sel); // the (still-)primary, moved first
            for e in extras {
                self.editor.sel = e;
                f(self, shift);
                new_sels.push(self.editor.sel);
            }
            self.set_cursors(aperture_core::multicursor::Cursors::new(new_sels));
        }
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

    /// Replace the buffer with `path` from disk. Returns whether it did.
    ///
    /// REFUSES while the current buffer has unsaved edits and `path` is a
    /// different file: this used to reopen unconditionally, so F12 into
    /// another file, the file picker, a project-search hit or a symbol jump
    /// all silently threw away whatever had not been saved. The same request
    /// again goes through (the status line says so), which keeps "yes, drop
    /// them" one repeat away instead of behind a dialog.
    pub fn open_file(&mut self, path: PathBuf, now_ms: u64) -> bool {
        // The SAME file counts too: reopening it reloads from disk, which is
        // a revert, and the picker offers the current file like any other.
        let other = self.path.as_ref() != Some(&path);
        if self.dirty && self.discard_armed.as_ref() != Some(&path) {
            let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "this buffer".into());
            let what = if other { "discard them" } else { "reload it from disk" };
            self.set_status(&format!("{name} has unsaved changes: Ctrl+S to save, or repeat to {what}"), now_ms);
            self.discard_armed = Some(path);
            return false;
        }
        self.discard_armed = None;
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                self.lang = Language::from_path(&path);
                self.syntax.set_language(self.lang);
                self.editor = Editor::from_str(&text);
                self.discard_extra_cursors(); // a whole new Editor; old offsets mean nothing
                self.path = Some(path);
                self.dirty = false;
                // A DIFFERENT FILE HAS A DIFFERENT HISTORY. The repo, the
                // file's repo-relative path and its timeline were computed
                // once, in `new`, for whatever file the app started on, and
                // never again: after Ctrl+O the gutter diffed the old file,
                // Ctrl+, loaded the OLD file's revisions into this buffer,
                // and a session started on a scratch buffer never learned it
                // was in a repo at all (so the workspace root, the picker and
                // bucket storage all fell back to the file's directory).
                self.rev_ix = None;
                self.live_text = None;
                self.git = self.path.as_deref().and_then(gitspace::discover);
                self.revs = match &self.git {
                    Some((r, rel)) => gitspace::history(r, rel, 48),
                    None => Vec::new(),
                };
                self.refresh_git();
                self.ghost.dismiss();
                self.debounce.cancel();
                self.cam.scroll_y.snap(0.0);
                self.follow_caret = false;
                self.set_status("opened", now_ms);
                self.start_lsp_for_current_file(&text);
                true
            }
            Err(e) => {
                self.set_status(&format!("open failed: {e}"), now_ms);
                false
            }
        }
    }

    // ── navigation path (aperture_core::nav) ───────────────────────────────

    /// Where the caret is, as the nav path names it: the innermost outline
    /// item around it (a file with no grammar, or code outside every item,
    /// is the FILE), and the exact line/column as the site.
    pub fn nav_here(&mut self) -> Option<(aperture_core::entity::EntityId, aperture_core::entity::Site)> {
        use aperture_core::entity::{EntityId, Site};
        let path = self.path.clone()?;
        let root = self.workspace_root();
        let rel = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let pos = self.editor.buffer.char_to_pos(self.editor.sel.head);
        // The SITE carries the absolute path: it is what back/forward open,
        // and `workspace_root()` is per-file outside a git repo, so a
        // relative path recorded in one directory resolves against another
        // one's root later. The entity stays workspace-relative -- it is an
        // identity and a label, not a place to open.
        let site = Site::new(path.to_string_lossy().into_owned(), pos.line as u32, pos.col as u32);
        let item = self
            .syntax
            .outline(&self.editor.buffer)
            .into_iter()
            .filter(|it| it.line <= pos.line && pos.line <= it.end_line)
            .max_by_key(|it| it.line);
        let id = match item {
            Some(it) => EntityId::item(rel, format!("{:?}", it.kind).to_lowercase(), it.name),
            None => EntityId::file(rel),
        };
        Some((id, site))
    }

    /// Record the jump that just happened, from `from` (taken BEFORE it) to
    /// wherever the caret is now.
    fn nav_record(&mut self, from: Option<(aperture_core::entity::EntityId, aperture_core::entity::Site)>, rel: aperture_core::entity::Relation) {
        let (Some((f, fs)), Some((t, ts))) = (from, self.nav_here()) else { return };
        self.nav.push(aperture_core::nav::NavStep::new(f, rel, t).at(fs).landed(ts));
    }

    /// Put the caret at `line`/`col` (chars) in `path`, opening it when it is
    /// not the current file. Same file: move within the OPEN buffer, so
    /// unsaved edits there are kept. Returns false when the switch was
    /// refused (unsaved edits elsewhere).
    fn go_to_site(&mut self, path: PathBuf, line: usize, col: usize, now_ms: u64) -> bool {
        if self.path.as_ref() != Some(&path) && !self.open_file(path, now_ms) {
            return false;
        }
        let line = line.min(self.editor.buffer.len_lines().saturating_sub(1));
        let len = self.editor.buffer.line(line).trim_end_matches(['\n', '\r']).chars().count();
        let at = self.editor.buffer.pos_to_char(aperture_core::buffer::Pos::new(line, col.min(len)));
        self.discard_extra_cursors();
        self.editor.sel = Selection::caret(at);
        self.follow_caret = true;
        true
    }

    /// Alt+Left (`back`) / Alt+Right: return to where a jump started — the
    /// exact callsite — or retake it to where it landed.
    pub fn nav_go(&mut self, back: bool, now_ms: u64) {
        let step = if back { self.nav.back().cloned() } else { self.nav.forward().cloned() };
        let Some(step) = step else {
            self.set_status(if back { "nothing to go back to" } else { "nothing ahead" }, now_ms);
            return;
        };
        let Some(site) = (if back { step.site.clone() } else { step.landing.clone() }) else {
            self.set_status("that step has no position recorded", now_ms);
            return;
        };
        let path = PathBuf::from(&site.path);
        let path = if path.is_absolute() { path } else { self.workspace_root().join(path) };
        if !self.go_to_site(path, site.line as usize, site.col as usize, now_ms) {
            // Refused (unsaved edits): the cursor did not really move.
            if back { self.nav.forward(); } else { self.nav.back(); }
            return;
        }
        let crumb = self.nav.breadcrumb(4);
        self.set_status(&if crumb.is_empty() { format!("back at {}", step.from.label()) } else { crumb }, now_ms);
    }

    /// A palette jump (file picker, project search, symbols, route): open,
    /// land, and record the step so Alt+Left comes back.
    pub fn palette_jump(&mut self, path: PathBuf, line: Option<usize>, rel: aperture_core::entity::Relation, now_ms: u64) {
        let from = self.nav_here();
        let ok = match line {
            Some(l) => self.go_to_site(path, l, 0, now_ms),
            None => self.open_file(path, now_ms),
        };
        if ok {
            self.nav_record(from, rel);
        }
    }

    // ── buckets (aperture_core::bucket) ─────────────────────────────────────

    /// Where buckets live: git refs (`refs/aperture/bucket/*`) when the
    /// file is in a repo, else a directory under the user's data dir keyed
    /// by the workspace root — never a file dropped into the project.
    fn bucket_store(&self) -> Box<dyn aperture_core::bucket::Store + '_> {
        use aperture_core::bucket::{slug, FileStore};
        match &self.git {
            Some((r, _)) => Box::new(aperture_core::gitspace::GitStore::new(r)),
            None => {
                let base = std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
                    .unwrap_or_else(std::env::temp_dir);
                let key = slug(&self.workspace_root().to_string_lossy());
                Box::new(FileStore::new(base.join("aperture/buckets").join(key)))
            }
        }
    }

    /// The loaded workspace, (re)loading it when absent or for another root.
    fn bucket_ws(&mut self) -> &mut aperture_core::bucket::Workspace {
        let root = self.workspace_root();
        if self.buckets.as_ref().map(|w| &w.root) != Some(&root) {
            let (ws, _skipped) = aperture_core::bucket::Workspace::load(root, &*self.bucket_store());
            self.buckets = Some(ws);
        }
        self.buckets.as_mut().expect("just loaded")
    }

    fn save_bucket(&mut self, i: usize, now_ms: u64) {
        let Some(ws) = self.buckets.clone() else { return };
        let r = ws.save(i, &*self.bucket_store());
        if let Err(e) = r {
            self.set_status(&format!("bucket not saved: {e}"), now_ms);
        }
    }

    /// The caret's place as a bucket entry: the innermost item, or the file.
    fn entry_here(&mut self) -> Option<aperture_core::bucket::Entry> {
        use aperture_core::bucket::Entry;
        let path = self.path.clone()?;
        let rel = self.bucket_ws().rel(&path);
        Some(match self.nav_here() {
            Some((aperture_core::entity::EntityId::Item { kind, name, .. }, _)) => Entry { kind: Some(kind), ..Entry::symbol(rel, name) },
            _ => Entry::file(rel),
        })
    }

    /// The commands aperture offers on the on-screen keyboard: the
    /// navigation that has a shortcut on a desktop and no key on a phone.
    /// Each is the chord the editor already binds, so the keyboard sends it
    /// like any key and nothing new has to be handled here.
    pub fn strip_actions() -> Vec<keyboard_strip::Action> {
        use keyboard_strip::Action;
        vec![
            Action::new("‹", "Alt+Left", "back to where the jump started"),
            Action::new("›", "Alt+Right", "forward again"),
            Action::new("Def", "F12", "go to definition"),
            Action::new("Callers", "Shift+F12", "who calls this"),
            Action::new("Symbols", "Ctrl+t", "go to symbol in workspace"),
            Action::new("Buckets", "Ctrl+k", "buckets"),
            Action::new("Find", "Ctrl+f", "find in file"),
            Action::new("Cmds", "Ctrl+p", "command palette"),
        ]
    }

    /// Tell the keyboard when focus changes: our strip on focus, cleared on
    /// blur. Only on a CHANGE — this runs every frame. A send that fails (no
    /// keyboard running: every desktop) is fine and silent.
    pub fn sync_keyboard_strip(&mut self, focused: bool) {
        if !self.strip_enabled || self.strip_sent == Some(focused) {
            return;
        }
        self.strip_sent = Some(focused);
        let msg = if focused {
            keyboard_strip::Msg::Show { app: "aperture".into(), pid: std::process::id(), actions: Self::strip_actions() }
        } else {
            keyboard_strip::Msg::Clear { app: "aperture".into() }
        };
        let _ = keyboard_strip::send(&msg);
    }

    /// Ctrl+K.
    pub fn open_buckets(&mut self, _now_ms: u64) {
        let active = self.bucket_ws().active;
        self.bucket_delete_armed = false;
        self.palette = Some(crate::palette_ui::PaletteState::buckets(active));
    }

    /// The rows the Bucket palette shows for `open` (None = the list) and
    /// the typed `filter`. Resolves each entry by NAME against its file's
    /// outline (the open buffer for the current file), so an entry follows
    /// its function when the function moves; one that no longer resolves
    /// is listed as such, never dropped.
    pub fn bucket_rows(&mut self, open: Option<usize>, filter: &str) -> Vec<crate::palette_ui::BucketRow> {
        use crate::palette_ui::{BucketAct, BucketRow};
        use aperture_core::bucket::{locate, Located};
        let f = filter.trim().to_lowercase();
        let ws = self.bucket_ws().clone();
        let row = |label: String, detail: String, act: BucketAct| BucketRow { label, detail, act, removable: None };
        let Some(bi) = open.filter(|i| *i < ws.buckets.len()) else {
            let mut rows: Vec<BucketRow> = ws
                .buckets
                .iter()
                .enumerate()
                .filter(|(_, b)| f.is_empty() || b.name.to_lowercase().contains(&f))
                .map(|(i, b)| row(b.name.clone(), format!("{} place{}", b.entries.len(), if b.entries.len() == 1 { "" } else { "s" }), BucketAct::Open(i)))
                .collect();
            let exact = ws.buckets.iter().any(|b| b.slug() == aperture_core::bucket::slug(filter.trim()));
            if !f.is_empty() && !exact {
                rows.insert(0, row(format!("+ New bucket \"{}\"", filter.trim()), "Enter creates it".into(), BucketAct::Create(filter.trim().to_string())));
            }
            if rows.is_empty() {
                rows.push(row("No buckets yet".into(), "type a name to make one".into(), BucketAct::None));
            }
            return rows;
        };
        let b = &ws.buckets[bi];
        let mut rows = vec![row("‹ All buckets".into(), b.name.clone(), BucketAct::Back)];
        if let Some(entry) = self.entry_here() {
            let label = entry.symbol.clone().unwrap_or_else(|| entry.path.clone());
            if b.contains(&entry) {
                rows.push(row(format!("{label} is in this bucket"), "the item under the caret".into(), BucketAct::None));
            } else {
                rows.push(row(format!("+ Add {label}"), "the item under the caret".into(), BucketAct::AddHere));
            }
        }
        let trail = self.nav.visited().len();
        if trail > 0 {
            rows.push(row(format!("+ Keep the jump trail ({trail} place{})", if trail == 1 { "" } else { "s" }), self.nav.breadcrumb(4), BucketAct::KeepTrail));
        }
        let mut outlines: std::collections::HashMap<PathBuf, Vec<(String, String, usize)>> = Default::default();
        for (ei, e) in b.entries.iter().enumerate() {
            let abs = {
                let p = PathBuf::from(&e.path);
                if p.is_absolute() { p } else { ws.root.join(p) }
            };
            let label = match &e.symbol {
                Some(s) => format!("{s}  ·  {}", e.path),
                None => e.path.clone(),
            };
            if !f.is_empty() && !label.to_lowercase().contains(&f) {
                continue;
            }
            let outline = outlines.entry(abs.clone()).or_insert_with(|| {
                if self.path.as_ref() == Some(&abs) {
                    self.syntax.outline(&self.editor.buffer).into_iter().map(|it| (format!("{:?}", it.kind).to_lowercase(), it.name, it.line)).collect()
                } else {
                    aperture_core::workspace_symbols::index_files(&ws.root, std::slice::from_ref(&abs), 1)
                        .into_iter()
                        .map(|s| (format!("{:?}", s.item.kind).to_lowercase(), s.item.name, s.item.line))
                        .collect()
                }
            });
            let triples: Vec<(&str, &str, usize)> = outline.iter().map(|(k, n, l)| (k.as_str(), n.as_str(), *l)).collect();
            let (detail, act) = if !abs.exists() {
                ("file is gone".to_string(), BucketAct::None)
            } else {
                match locate(e, &triples) {
                    Located::Line(l) => (format!("line {}", l + 1), BucketAct::Jump { path: abs.clone(), line: l }),
                    Located::FileOnly => ("file".to_string(), BucketAct::Jump { path: abs.clone(), line: 0 }),
                    Located::Unresolved => ("not found in the file (renamed?)".to_string(), BucketAct::Jump { path: abs.clone(), line: 0 }),
                }
            };
            rows.push(BucketRow { label, detail, act, removable: Some(ei) });
        }
        rows.push(row(
            if self.bucket_delete_armed { "Delete this bucket? (choose again)".into() } else { "Delete this bucket".into() },
            "its entries are references; no code is touched".into(),
            BucketAct::DeleteBucket,
        ));
        rows
    }

    /// Carry out a Bucket row. Returns whether the palette should close.
    pub fn bucket_act(&mut self, act: crate::palette_ui::BucketAct, now_ms: u64) -> bool {
        use crate::palette_ui::{BucketAct, PaletteState};
        use aperture_core::bucket::Entry;
        let open = self.palette.as_ref().and_then(|p| p.bucket_open);
        if !matches!(act, BucketAct::DeleteBucket) {
            self.bucket_delete_armed = false;
        }
        match act {
            BucketAct::None => false,
            BucketAct::Open(i) => {
                self.bucket_ws().active = Some(i);
                self.palette = Some(PaletteState::buckets(Some(i)));
                false
            }
            BucketAct::Create(name) => {
                let i = self.bucket_ws().open(&name);
                self.save_bucket(i, now_ms);
                self.palette = Some(PaletteState::buckets(Some(i)));
                false
            }
            BucketAct::Back => {
                self.palette = Some(PaletteState::buckets(None));
                false
            }
            BucketAct::AddHere => {
                let Some(bi) = open else { return false };
                let Some(entry) = self.entry_here() else { return false };
                if let Some(b) = self.bucket_ws().buckets.get_mut(bi) {
                    b.add(entry);
                }
                self.save_bucket(bi, now_ms);
                false
            }
            BucketAct::KeepTrail => {
                let Some(bi) = open else { return false };
                // nav's entity paths are workspace-relative, like bucket paths.
                let mut added = 0;
                for id in self.nav.visited() {
                    let e = match id {
                        aperture_core::entity::EntityId::Item { path, kind, name } => Entry { kind: Some(kind), ..Entry::symbol(path, name) },
                        aperture_core::entity::EntityId::File { path } => Entry::file(path),
                        _ => continue,
                    };
                    if let Some(b) = self.bucket_ws().buckets.get_mut(bi) {
                        if !b.contains(&e) {
                            b.add(e);
                            added += 1;
                        }
                    }
                }
                self.save_bucket(bi, now_ms);
                self.set_status(&format!("kept {added} place{} from the trail", if added == 1 { "" } else { "s" }), now_ms);
                false
            }
            BucketAct::Jump { path, line } => {
                self.palette_jump(path, Some(line), aperture_core::entity::Relation::Opened, now_ms);
                true
            }
            BucketAct::Remove(ei) => {
                let Some(bi) = open else { return false };
                if let Some(b) = self.bucket_ws().buckets.get_mut(bi) {
                    b.remove(ei);
                }
                self.save_bucket(bi, now_ms);
                false
            }
            BucketAct::DeleteBucket => {
                let Some(bi) = open else { return false };
                if !self.bucket_delete_armed {
                    self.bucket_delete_armed = true;
                    return false;
                }
                self.bucket_delete_armed = false;
                let mut ws = self.bucket_ws().clone();
                let r = ws.close(bi, &*self.bucket_store());
                self.buckets = Some(ws);
                if let Err(e) = r {
                    self.set_status(&format!("bucket not deleted: {e}"), now_ms);
                }
                self.palette = Some(PaletteState::buckets(None));
                false
            }
        }
    }

    /// [`Self::palette_jump`] to an exact column (a call site, a reference).
    pub fn palette_jump_at(&mut self, path: PathBuf, line: usize, col: usize, rel: aperture_core::entity::Relation, now_ms: u64) {
        let from = self.nav_here();
        if self.go_to_site(path, line, col, now_ms) {
            self.nav_record(from, rel);
        }
    }

    /// Absolute `file://` URI for `path` — LSP addresses documents by URI,
    /// never by a bare path, and a relative one would be ambiguous about
    /// which directory it's relative TO once it crosses the wire to a
    /// server process with its own cwd.
    fn file_uri(path: &std::path::Path) -> String {
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        format!("file://{}", abs.display())
    }

    /// Best-effort: spawn `rust-analyzer` for the just-opened file if it's
    /// Rust and nothing has already been started for a different file this
    /// session. Silent on failure (no `rust-analyzer` on `PATH` is the
    /// overwhelmingly common case in any environment that hasn't installed
    /// it) — this is an ADDITIONAL tier, never something the editor depends
    /// on working. Only Rust is wired up; every other language leaves
    /// `self.lsp` as `None`, same as before this method existed.
    fn start_lsp_for_current_file(&mut self, text: &str) {
        if self.lang != Language::Rust {
            return;
        }
        let Some(path) = self.path.clone() else { return };
        let uri = Self::file_uri(&path);
        let root_uri = Self::file_uri(&self.workspace_root());
        if self.lsp.is_none() {
            // Tried and failed for this root already: do not respawn on
            // every file open (the handshake result arrives in `poll_lsp`).
            if self.lsp_failed_root.as_deref() == Some(root_uri.as_str()) {
                return;
            }
            match crate::lsp_worker::LspWorker::spawn("rust-analyzer", &[], &root_uri) {
                Ok(w) => {
                    self.lsp = Some(w);
                    self.lsp_root = Some(root_uri.clone());
                }
                Err(_) => return, // no server available — stay on the local-only tier
            }
        }
        self.lsp_doc_version = 1;
        self.lsp_uri = Some(uri.clone());
        self.lsp_last_synced_text = text.to_string();
        if let Some(lsp) = &self.lsp {
            lsp.open(uri, text.to_string());
        }
    }

    /// Inverse of `file_uri` — strips the `file://` prefix this crate's own
    /// URIs always use. Not a general URI parser (no percent-decoding, no
    /// handling of a `file://host/path` authority): every URI this method
    /// ever sees either came from `file_uri` itself or from a server
    /// answering a request THIS client made with one of those URIs, so the
    /// simple cases are the only ones that can actually occur here.
    fn path_from_file_uri(uri: &str) -> Option<std::path::PathBuf> {
        uri.strip_prefix("file://").map(std::path::PathBuf::from)
    }

    /// F12: ask the LSP worker where the symbol under the caret is defined.
    /// No-op, silently, when no worker is running — same "this tier simply
    /// isn't available" handling every other LSP entry point uses.
    fn go_to_definition(&mut self, now_ms: u64) {
        let (Some(lsp), Some(uri)) = (&self.lsp, &self.lsp_uri) else {
            self.set_status("go to definition: no language server running", now_ms);
            return;
        };
        self.lsp_definition_gen += 1;
        let p = self.editor.caret_pos();
        let line_text = self.editor.buffer.line(p.line);
        let utf16_col = aperture_core::width::char_col_to_utf16(&line_text, p.col);
        lsp.request_definition(uri.clone(), p.line as u32, utf16_col, self.lsp_definition_gen);
    }

    /// Shift+F12: who calls the symbol at the caret. Answered in
    /// `poll_lsp` -> [`Self::show_callers`].
    pub fn find_callers(&mut self, now_ms: u64) {
        let (Some(lsp), Some(uri)) = (&self.lsp, &self.lsp_uri) else {
            self.set_status("callers: no language server running", now_ms);
            return;
        };
        self.lsp_callers_gen += 1;
        let p = self.editor.caret_pos();
        let line_text = self.editor.buffer.line(p.line);
        let utf16_col = aperture_core::width::char_col_to_utf16(&line_text, p.col);
        lsp.request_callers(uri.clone(), p.line as u32, utf16_col, self.lsp_callers_gen);
        self.set_status("callers: asking the language server…", now_ms);
    }

    /// Turn a callers answer into rows: one per CALL SITE (a caller that
    /// calls twice is two places you might want), columns converted from
    /// UTF-16 against the target line's real text. One row jumps straight
    /// there; several open [`crate::palette_ui::Mode::Callers`].
    pub fn show_callers(&mut self, outcome: crate::lsp_worker::CallersOutcome, now_ms: u64) {
        use crate::palette_ui::{CallerHit, PaletteState};
        if let Some(e) = &outcome.error {
            if outcome.calls.is_empty() && outcome.refs.is_empty() {
                self.set_status(&format!("callers: {e}"), now_ms);
                return;
            }
        }
        let root = self.workspace_root();
        // Line text for a (path, line): the OPEN buffer for the current
        // file (it may have unsaved edits the server has seen), disk for
        // the rest, read once per file.
        let mut files: std::collections::HashMap<PathBuf, Vec<String>> = Default::default();
        let current = self.path.clone();
        let mut line_of = |path: &PathBuf, line: usize, this: &Self| -> String {
            if current.as_ref() == Some(path) {
                return this.editor.buffer.line(line.min(this.editor.buffer.len_lines().saturating_sub(1))).to_string();
            }
            let lines = files.entry(path.clone()).or_insert_with(|| {
                std::fs::read_to_string(path).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default()
            });
            lines.get(line).cloned().unwrap_or_default()
        };
        let mut hits = Vec::new();
        let rel = |p: &PathBuf| p.strip_prefix(&root).unwrap_or(p).to_string_lossy().into_owned();
        let are_calls = !outcome.calls.is_empty();
        if are_calls {
            for call in &outcome.calls {
                let Some(path) = Self::path_from_file_uri(&call.uri) else { continue };
                let sites: Vec<(u32, u32)> = if call.sites.is_empty() { vec![(call.line, call.character)] } else { call.sites.clone() };
                for (l, c) in sites {
                    let text = line_of(&path, l as usize, self);
                    hits.push(CallerHit {
                        col: aperture_core::width::utf16_col_to_char(&text, c),
                        line: l as usize,
                        title: call.name.clone(),
                        detail: format!("{}:{}  {}", rel(&path), l + 1, text.trim()),
                        path: path.clone(),
                    });
                }
            }
        } else {
            for loc in &outcome.refs {
                let Some(path) = Self::path_from_file_uri(&loc.uri) else { continue };
                let text = line_of(&path, loc.line as usize, self);
                hits.push(CallerHit {
                    col: aperture_core::width::utf16_col_to_char(&text, loc.character),
                    line: loc.line as usize,
                    title: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    detail: format!("{}:{}  {}", rel(&path), loc.line + 1, text.trim()),
                    path: path.clone(),
                });
            }
        }
        use aperture_core::entity::Relation;
        let relation = if are_calls { Relation::CalledBy } else { Relation::ReferencedBy };
        match hits.len() {
            0 => self.set_status("callers: none found", now_ms),
            1 => {
                let h = hits.remove(0);
                self.palette_jump_at(h.path, h.line, h.col, relation, now_ms);
                self.set_status(&format!("{} — the only {} (Alt+Left returns)", h.title, if are_calls { "caller" } else { "reference" }), now_ms);
            }
            n => {
                self.set_status(&format!("{n} {}", if are_calls { "call sites" } else { "references" }), now_ms);
                self.palette = Some(PaletteState::callers(hits, are_calls));
            }
        }
    }

    /// Ctrl+I: ask the LSP worker for a type signature / doc summary at
    /// the caret. Same "silently unavailable" handling as go-to-definition
    /// when no worker is running.
    fn request_hover(&mut self, now_ms: u64) {
        let (Some(lsp), Some(uri)) = (&self.lsp, &self.lsp_uri) else {
            self.set_status("hover: no language server running", now_ms);
            return;
        };
        self.lsp_hover_gen += 1;
        let p = self.editor.caret_pos();
        let line_text = self.editor.buffer.line(p.line);
        let utf16_col = aperture_core::width::char_col_to_utf16(&line_text, p.col);
        lsp.request_hover(uri.clone(), p.line as u32, utf16_col, self.lsp_hover_gen);
    }

    /// Apply an arrived go-to-definition answer: jump to the FIRST location
    /// (the common case is exactly one; `Location`'s own doc explains why
    /// the client returns every one rather than picking for the caller —
    /// this is the one place in the shell that has to pick, since a caret
    /// can only go one place at a time. A palette-driven "choose among N"
    /// for the rare multi-result case is future work, not attempted here).
    fn jump_to_lsp_definition(&mut self, loc: &aperture_lsp::Location, now_ms: u64) {
        let Some(target_path) = Self::path_from_file_uri(&loc.uri) else {
            self.set_status("go to definition: couldn't resolve the target path", now_ms);
            return;
        };
        // Same file: move the caret in the buffer that's ALREADY open,
        // rather than reopening it — reopening would discard any unsaved
        // edits, which `open_file` always does (fresh `Editor::from_str`
        // straight from disk).
        let same_file = self.path.as_ref().is_some_and(|p| Self::file_uri(p) == loc.uri);
        let from = self.nav_here();
        if !same_file && !self.open_file(target_path, now_ms) {
            return; // unsaved edits here; open_file said so
        }
        let line = (loc.line as usize).min(self.editor.buffer.len_lines().saturating_sub(1));
        let line_text = self.editor.buffer.line(line);
        let col = aperture_core::width::utf16_col_to_char(&line_text, loc.character);
        let at = self.editor.buffer.pos_to_char(aperture_core::buffer::Pos::new(line, col));
        self.editor.sel = Selection::caret(at);
        self.follow_caret = true;
        self.nav_record(from, aperture_core::entity::Relation::References);
        self.set_status("jumped to definition (Alt+Left returns)", now_ms);
    }

    // ── AI wiring, called from the view's frame tick ────────────────────────

    /// Fire the debounce if due; send at most one request.
    pub fn tick_completion(&mut self, now_ms: u64) {
        if self.debounce.fire(now_ms) {
            self.trigger_completion_now(now_ms);
        }
    }

    /// Ctrl+Space and the palette's "complete now" both land here.
    /// Run one modal keystroke. Returns true when it was consumed, so the
    /// caller knows not to insert the character as text.
    ///
    /// Only live while typewriter mode is on: with it off there is no
    /// modal layer at all and every key is text, which is what makes the
    /// feature safe to have — you cannot end up trapped in Normal mode in
    /// an editor you thought was ordinary.
    pub fn helix_key(&mut self, ch: char, shift: bool, now_ms: u64) -> bool {
        use aperture_core::helix::{self, Act, Edge, Mode, Motion};
        if !self.typewriter_mode || self.helix == Mode::Insert {
            return false;
        }
        let act = helix::resolve(self.helix, ch, shift);
        if act == Act::Pass {
            return false;
        }
        // The modal engine predates multi-cursor and mutates `Editor`
        // directly (Delete/Paste/Insert/OpenLine below) with no notion of
        // `extra_cursors`. Rather than teach every operator to fan out,
        // collapse once here — pure navigation (`Act::Move`) doesn't
        // corrupt anything, but there's no user-visible harm in collapsing
        // for it too, and a narrower "only for mutating acts" condition
        // would be one more thing to keep in sync as operators are added.
        self.discard_extra_cursors();
        let extend = self.helix == Mode::Visual;
        match &act {
            Act::Move(m) => match m {
                Motion::Left => self.editor.move_left(extend),
                Motion::Right => self.editor.move_right(extend),
                Motion::Up => self.editor.move_vertical(-1, extend),
                Motion::Down => self.editor.move_vertical(1, extend),
                Motion::WordNext => self.editor.move_word(1, extend),
                Motion::WordPrev => self.editor.move_word(-1, extend),
                Motion::LineStart => self.editor.move_home(extend),
                Motion::LineEnd => self.editor.move_end(extend),
                Motion::DocStart => self.editor.move_doc(false, extend),
                Motion::DocEnd => self.editor.move_doc(true, extend),
            },
            Act::Delete => {
                if self.editor.sel.is_caret() {
                    self.editor.delete_forward(now_ms);
                } else {
                    let (a, b) = (self.editor.sel.min(), self.editor.sel.max());
                    self.yank = self.editor.selected_text();
                    self.editor.replace_range(a, b, "");
                }
                self.after_edit(now_ms);
            }
            Act::Yank => {
                if !self.editor.sel.is_caret() {
                    self.yank = self.editor.selected_text();
                    self.set_status(&format!("yanked {} chars", self.yank.chars().count()), now_ms);
                }
            }
            Act::Paste => {
                if !self.yank.is_empty() {
                    let t = self.yank.clone();
                    self.editor.insert_text(&t, now_ms);
                    self.after_edit(now_ms);
                }
            }
            Act::Insert { at } => {
                // From a SELECTION these mean its edges, which is why the
                // edge travels with the act rather than being a nudge the
                // caller has to apply in the right direction.
                if !self.editor.sel.is_caret() {
                    let to = match at {
                        Edge::Before => self.editor.sel.min(),
                        Edge::After => self.editor.sel.max(),
                    };
                    self.editor.sel = aperture_core::editor::Selection::caret(to);
                } else if *at == Edge::After {
                    self.editor.move_right(false);
                }
            }
            Act::OpenLine { above } => {
                if *above {
                    // Was a bare `insert_text("\n")` -- zero indent at
                    // all, unlike `o` (below), which at least copied the
                    // current line's indent even before this session's
                    // smart-indent fix. Shift-`o` opens a SIBLING line
                    // before the current one, not a new nested block, so
                    // it matches the current line's own indent rather
                    // than deepening the way `o` after an opener now does.
                    //
                    // KNOWN GAP, not attempted here: if the current line
                    // is itself a closer (`}`), matching ITS indent is
                    // the outer level, not the inner one the new line is
                    // semantically inside of. Getting that right needs
                    // finding the closer's matching opener (a real
                    // bracket-matching scan backward, not a one-line
                    // lookup), which is a bigger feature than this fix
                    // makes -- Shift-O above a lone `}` still lands the
                    // new line at the closer's own (outer) indent today.
                    let line = self.editor.buffer.char_to_pos(self.editor.sel.head).line;
                    let indent = self.editor.line_indent(line);
                    self.editor.move_home(false);
                    let p = self.editor.sel.head;
                    self.editor.insert_text(&format!("{indent}\n"), now_ms);
                    self.editor.sel = Selection::caret(p + indent.chars().count());
                } else {
                    self.editor.move_end(false);
                    self.editor.newline_smart(now_ms);
                }
                self.after_edit(now_ms);
            }
            Act::InsertAtLineEdge { end } => {
                // Unconditional line edge, NOT the selection's edge —
                // that is what distinguishes Shift-I/A from i/a. A
                // selection is collapsed by the move itself (extend =
                // false), which is what Helix does too: Shift-I from a
                // visual selection puts you at the line start in Insert,
                // not at the selection start.
                if *end {
                    self.editor.move_end(false);
                } else {
                    self.editor.move_home(false);
                }
            }
            Act::SelectLine => {
                // Helix's `x`: select the whole line the caret is on, and
                // extend onto the next line when a line is already
                // selected. Built from move_home/move_end with `extend`
                // rather than a new Editor primitive, because that is
                // exactly what "select this line" IS in terms the editor
                // already has, and a second implementation of line bounds
                // would be one more thing to keep in step with them.
                if self.editor.sel.is_caret() {
                    self.editor.move_home(false);
                    self.editor.move_end(true);
                } else {
                    // Already a selection: extend a line further down,
                    // which is what a repeated `x` means.
                    self.editor.move_vertical(1, true);
                    self.editor.move_end(true);
                }
            }
            Act::Search => {
                // The SAME bar Ctrl+F opens, not a second search surface.
                // `FindState` already does incremental matching, next/prev
                // with wraparound, match highlighting, history and replace;
                // a `/`-specific searcher would have been a second
                // implementation of all of it, drifting from this one the
                // first time either changed. `open_find` also seeds the
                // query from the selection, which is the right behaviour
                // for `/` too — Helix's own `/` after a selection is
                // usually "find more of this".
                self.open_find();
            }
            Act::CommandPalette => {
                self.open_palette();
            }
            Act::EnterVisual | Act::EnterNormal | Act::Pass => {}
            Act::Route => {
                // `f` is the route palette, not find-char: this editor's
                // useful "go somewhere" verb already exists and giving the
                // best key the weaker meaning would be the worse trade.
                // The same method Ctrl+G uses. `f` is bound to routing
                // rather than find-char on purpose (see helix::resolve),
                // and it must reach the identical entry point or the two
                // keys would drift into two behaviours.
                self.open_go_to_route();
            }
            Act::CycleCompletionSpeed => {
                let next = helix::next_debounce(self.debounce.idle_ms);
                self.debounce.idle_ms = next;
                self.set_status(
                    &format!("completion debounce: {next} ms{}", match next {
                        250 => " (as you type — warmest)",
                        1250 => " (only when you stop — coolest)",
                        _ => "",
                    }),
                    now_ms,
                );
            }
        }
        self.helix = helix::next_mode(self.helix, &act);
        true
    }

    /// Run a command chosen from the bottom strip, and record the use.
    ///
    /// Recording is the point of the strip: it leads with what you just
    /// used, so a command run from it has to feed that list too — or the
    /// strip would only ever learn from the palette and the thing you
    /// press most would drift off it.
    ///
    /// Unknown ids are IGNORED rather than silently doing something
    /// close: a stale entry (a command removed since it was recorded) is
    /// a real possibility, and guessing is worse than nothing happening.
    pub fn run_strip_command(&mut self, id: &str, now_ms: u64) {
        // Matched on the LABEL, which is what the strip stores as its id.
        // The palette has no separate command id, and inventing one here
        // would be a second naming scheme to keep in step with the first.
        let Some((label, cmd)) = crate::palette_ui::command_list()
            .into_iter()
            .find(|(l, _)| l == id)
        else {
            self.set_status("that command is no longer available", now_ms);
            return;
        };
        self.cmd_strip
            .used(uikit_egui::cmdstrip::Cmd::new(&label, &label));
        crate::palette_ui::apply(cmd, self, now_ms);
    }

    /// Store the menu a reply produced.
    ///
    /// The first insertable block is already the ghost, so it is dropped
    /// here — offering it twice, once on Tab and once on a letter, would
    /// make one suggestion look like two.
    pub fn set_completion_menu(
        &mut self,
        blocks: Vec<aperture_complete::multipass::Block>,
        prose: Vec<String>,
    ) {
        // A NEW REPLY ENDS THE OLD ACCEPT. Without this, Tab would carry
        // on typing out the previous block against a ghost that has been
        // replaced — inserting text from a suggestion the person can no
        // longer see, which is the worst failure this feature could have.
        self.typeout = None;
        let mut took_ghost = false;
        self.menu = blocks
            .into_iter()
            .filter(|b| {
                if b.kind.insertable() && !took_ghost {
                    took_ghost = true;
                    return false;
                }
                true
            })
            .collect();
        self.menu_prose = prose;
    }

    /// Accept the block bound to `key`.
    ///
    /// Returns true when something was inserted, so the caller knows the
    /// keystroke was CONSUMED and must not also reach the buffer — the
    /// same ordering that caused the Tab fight.
    ///
    /// Only insertable blocks carry keys (`multipass::assign_keys`), so
    /// there is no path here that can insert an elision. Enforced by the
    /// assignment rather than re-checked here: two places deciding one
    /// safety rule is how they drift apart.
    pub fn accept_menu_key(&mut self, key: char, now_ms: u64) -> bool {
        let Some(block) = self.menu.iter().find(|b| b.key == Some(key)).cloned() else {
            return false;
        };
        self.editor.insert_text(&block.text, now_ms);
        self.after_edit(now_ms);
        // Spent: the blocks were alternatives to each other, and leaving
        // them offered invites building a function from two mutually
        // exclusive halves.
        self.dismiss_menu();
        true
    }

    /// ESC: back to typing, nothing inserted.
    pub fn dismiss_menu(&mut self) {
        self.menu.clear();
        self.menu_prose.clear();
    }

    pub fn trigger_completion_now(&mut self, _now_ms: u64) {
        let ctx = self.fim_context();
        let Some(engine) = &self.engine else { return };
        if !self.ai_enabled {
            return;
        }
        // THE STUTTER. Nothing checked whether a request was already in
        // flight, so a typist faster than the model produced overlapping
        // requests — each landing and replacing the ghost with a
        // suggestion computed from an older buffer, which reads as text
        // flickering between two answers. The debounce alone does not
        // prevent it: it spaces requests by IDLE TIME, and a model slower
        // than the idle window still has one outstanding when the next
        // fires. Waiting for the pending one costs at most one round trip
        // and the newer request will be armed again by the next edit.
        if self.ghost.is_pending() {
            return;
        }
        // Only offer at a caret (not over a selection): a suggestion that
        // replaces a selection is a different feature (edit prediction).
        if !self.editor.sel.is_caret() {
            return;
        }
        let head = self.editor.sel.head;
        let full_before = self.editor.buffer.slice_to_string(0, head);
        // SCOPE THE CONTEXT. `before` is everything above the caret; the
        // scope decides how much of it the model actually sees. Sliced
        // here rather than inside make_request so the prefix the cache is
        // keyed on is the prefix that gets sent — the two must not be able
        // to disagree.
        let lang = match self.lang {
            Language::Rust => "rust",
            Language::Plain => "text",
        };
        let scoped_start = aperture_complete::scope::scope_start(
            &full_before, self.scope, self.line_scope_lines);
        // ANCHORED TRIM, not a sliding window. File scope on a large
        // file exceeds any sane budget, and the obvious fix ("keep the
        // last N bytes") moves the prompt's start on EVERY keystroke —
        // so no two consecutive prompts share a prefix and the KV cache
        // never hits. Quantising the start to a line-boundary grid keeps
        // it still while you type and jumps only when it must.
        let scoped = aperture_complete::kvplan::anchored_trim(
            &full_before[scoped_start..], PREFIX_BUDGET);
        // QUICK CONTEXT, prepended as a plain `// ` comment line — not a
        // new field on CompletionRequest. `path` grew the request's shape
        // last time and an eighth construction site outside this
        // workspace broke because of it (games/blackstar-rs's raid
        // forge). A comment is just more text in `prefix`, which every
        // existing provider and call site already handles by definition,
        // so this cannot repeat that mistake. Rust-only: a `//` header in
        // front of Markdown or plain text would read as literal text to
        // continue, not as metadata.
        let quick_ctx = if lang == "rust" {
            aperture_complete::scope::QuickContext::gather(scoped, &full_before).render()
        } else {
            String::new()
        };
        // VOLATILE LAST. The previous commit put this header at byte 0,
        // which is the worst place for it on a KV-cached engine: the LOC
        // figure changes whenever the file grows a line, so every such
        // keystroke changed the FIRST bytes of the prompt and invalidated
        // the entire cached prefix — a full re-prefill of the whole
        // context, with nothing to notice it by except the editor feeling
        // slow. `kvplan::assemble` puts it immediately above the caret's
        // line instead: same information, a few tokens of re-prefill
        // instead of all of them. See kvplan's module docs.
        let before = aperture_complete::kvplan::assemble(scoped, &quick_ctx);
        let after = self.editor.buffer.slice_to_string(head, self.editor.buffer.len_chars());
        // ONE REQUEST IN FLIGHT AT A TIME — the stutter fix. Nothing
        // checked this, so a slow model plus a fast typist produced
        // overlapping requests, each landing and REPLACING the ghost as it
        // arrived: the suggestion visibly flickered between two or three
        // answers to prompts you had already typed past. The generation
        // gate drops STALE answers, but only once they arrive, which is
        // too late to stop the flicker and wasted the GPU time either way.
        //
        // Skipping is right rather than queueing: the debounce re-arms on
        // the next edit, so the newest context still gets asked about the
        // moment the current answer lands. Queueing would guarantee the
        // work is done and thrown away.
        if self.ghost.is_pending() {
            return;
        }
        let mut req = make_request(&ctx, &before, &after, lang, self.editor.version, PREFIX_BUDGET, SUFFIX_BUDGET);
        req.path = self.path.clone(); // sibling reads + @read resolve here

        // ASK THE CACHE BEFORE THE MODEL. `req.prefix` is exactly what the
        // provider would send, so it is exactly the right key — caret
        // motion that changes no text produces the identical prefix and
        // is served here without a request, and typing along an accepted
        // suggestion is served from the tail the model already committed
        // to. Only a real miss reaches the engine.
        match self.prefix_cache.get(&req.prefix, &req.lang) {
            Some(aperture_complete::prefixcache::Hit::Exact(text))
            | Some(aperture_complete::prefixcache::Hit::Extended { completion: text, .. }) => {
                // Straight to the ghost, at the CURRENT version, so the
                // generation gate treats it exactly like a fresh answer.
                // Same door a real answer comes through, so the
                // generation gate and the version check apply identically
                // — a cached suggestion must not bypass the staleness
                // rules a fresh one obeys.
                self.ghost.request_sent(req.generation);
                if let Some((menu, prose)) = self.ghost.on_outcome(
                    req.generation, self.editor.version, &Ok(text), &self.complete_keys)
                {
                    self.set_completion_menu(menu, prose);
                }
                let (h, m) = self.prefix_cache.stats();
                self.status = Some((format!("completion from cache ({h} hit / {m} miss)"), 1_200));
                return;
            }
            None => {}
        }
        self.pending_prefix = Some((req.prefix.clone(), req.lang.clone()));
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
        if let Some((menu, prose)) =
            self.ghost.on_outcome(out.generation, self.editor.version, &out.result,
                                  &self.complete_keys)
        {
            self.set_completion_menu(menu, prose);
        }
        // FILE THE ANSWER. Keyed on the prefix the request was actually
        // built from (captured at dispatch), not on the buffer as it looks
        // now — by the time an answer lands the caret has often moved, and
        // keying on the current state would file it under something the
        // model never saw.
        if let (Ok(text), Some((prefix, lang))) = (&out.result, self.pending_prefix.clone()) {
            if !text.trim().is_empty() {
                self.prefix_cache.insert(&prefix, &lang, text);
            }
        }
        if self.ghost.is_shown() && !was_shown {
            self.ghost_since = Some(now_ms);
        }
        // A completion armed BEFORE Ctrl+D can still land after — `arm_
        // completion` never fires while multi-cursor typing, but an
        // already-in-flight request passes the generation gate regardless.
        // The ghost's own accept path (Tab / Ctrl+Tab / Ctrl+Right) calls
        // `editor.insert_text` directly, not `fan_out_insert`, so showing it
        // here would let a later Tab bypass every secondary cursor.
        if !self.extra_cursors.is_empty() {
            self.ghost.dismiss();
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

    pub(crate) fn open_palette(&mut self) {
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

    pub(crate) fn open_voice_command(&mut self) {
        // Same reasoning as `open_find_ex`: almost every VoiceCommand arm
        // mutates through a single-cursor method (undo, Editor::select_all,
        // goto_line, ...) with no notion of extra_cursors, so this mode
        // change is the same kind of fan-out-unaware entry point.
        self.discard_extra_cursors();
        self.palette = Some(crate::palette_ui::PaletteState::voice_command());
        self.ghost.dismiss();
        self.debounce.cancel();
    }

    /// Palette entry point for a typed stand-in for a voice transcript —
    /// see `aperture_core::voice`'s module doc for the honest scope (no
    /// microphone, no ASR; this is the intent-matching half only). Every
    /// arm calls an EXISTING method rather than duplicating its logic, so
    /// "voice save" and Ctrl+S can never disagree about what saving means.
    ///
    /// Returns clipboard text for Copy, same shape as `Effects::clipboard`
    /// from `handle()` — the caller is responsible for actually setting the
    /// platform clipboard (this crate is headless w.r.t. `arboard`/egui's
    /// own clipboard, same reason `Effects` exists at all).
    ///
    /// Single-speaker convenience: everything from this entry point shares
    /// one "again"/vocab history bucket, keyed `"local"`. A caller with a
    /// real multi-participant story (several people voice-driving one
    /// shared session) should call `run_voice_command_as` directly with
    /// each person's own identity instead — see that method's doc for why
    /// "again" specifically needs to know who's asking.
    pub fn run_voice_command(&mut self, utterance: &str, now_ms: u64) -> Option<String> {
        self.run_voice_command_as("local", utterance, now_ms)
    }

    /// Like `run_voice_command`, but attributed to `speaker` — whatever
    /// upstream identity a real pipeline hands this layer (a `SiteId`-
    /// shaped string is the natural choice, matching `aperture_collab`'s
    /// existing multi-participant identity model rather than inventing a
    /// second one). Two things become per-speaker rather than global:
    ///
    /// 1. "again"/"repeat that" replays THAT SPEAKER's own last command,
    ///    not whoever spoke most recently in the whole session — the one
    ///    piece of multi-user behavior that is silently wrong by default
    ///    (Bob saying "again" after Alice's command should not redo
    ///    Alice's edit under Bob's name).
    /// 2. The vocabulary itself is shared (everyone benefits from the same
    ///    project's symbol names being sayable), but resolution still
    ///    happens per-utterance, so a "go to compute total" ambiguity is
    ///    never influenced by what a DIFFERENT speaker said earlier.
    pub fn run_voice_command_as(&mut self, speaker: &str, utterance: &str, now_ms: u64) -> Option<String> {
        use aperture_core::voice::VoiceCommand as V;
        // Vocabulary buildup: whatever this file's own outline currently
        // knows about, refreshed on every command rather than cached —
        // a voice command is rare enough (a few per minute at most) that
        // re-deriving from the already-parsed tree costs nothing worth
        // guarding, and a stale vocabulary (missing a function someone
        // just wrote) would be a confusing, silent failure to navigate to
        // exactly the thing they just added.
        let vocab: Vec<String> = self.gather_symbols().iter().map(|(name, _)| name.clone()).collect();
        self.voice_session.set_vocab(vocab);
        match self.voice_session.handle(speaker, utterance) {
            V::Save => self.save(now_ms),
            V::Undo => self.editor.undo(),
            V::Redo => self.editor.redo(),
            V::SelectAll => self.editor.select_all(),
            V::Copy => return self.copy_selection(),
            V::Cut => {
                let clip = self.copy_selection();
                if !self.editor.sel.is_caret() {
                    let (a, b) = (self.editor.sel.min(), self.editor.sel.max());
                    self.editor.replace_range(a, b, "");
                    self.after_edit(now_ms);
                }
                return clip;
            }
            V::Paste => {
                // No clipboard SOURCE reaches this entry point (the platform
                // clipboard is read by the view layer's egui::Event::Paste,
                // not by anything this headless method can call into) — say
                // so rather than silently doing nothing, same honesty as
                // the Unknown arm below.
                self.set_status("voice: paste needs the palette closed — Ctrl+V works normally", now_ms);
            }
            V::NewLine => {
                self.editor.newline_smart(now_ms);
                self.after_edit(now_ms);
            }
            V::Escape => {
                self.dismiss_menu();
                self.ghost.dismiss();
                self.extra_cursors.clear();
            }
            V::Find(q) => {
                self.open_find();
                if let Some(f) = &mut self.find {
                    f.query = q;
                    f.recompute(&self.editor.buffer, self.editor.version);
                }
            }
            V::GoToLine(n) => {
                self.editor.goto_line(n.saturating_sub(1)); // spoken "line 1" is index 0
                self.follow_caret = true;
            }
            V::GoToSymbol(name) => {
                // Re-derive the line from the outline directly rather than
                // threading it through `gather_symbols`'s (name, sig) pairs
                // — those exist for the completion popup's display text and
                // were never meant to carry a jump target too; asking them
                // to would couple two unrelated callers' needs together.
                let line = self.syntax.outline(&self.editor.buffer).into_iter()
                    .find(|item| item.name == name)
                    .map(|item| item.line);
                match line {
                    Some(line) => {
                        self.editor.goto_line(line);
                        self.follow_caret = true;
                    }
                    None => self.set_status(
                        &format!("voice: \"{name}\" isn't in this file's outline anymore"), now_ms),
                }
            }
            V::AddNextCursor => self.add_next_cursor(now_ms),
            V::SelectAllOccurrences => self.select_all_cursor_occurrences(now_ms),
            V::Type(text) => {
                self.editor.insert_text(&text, now_ms);
                self.after_edit(now_ms);
            }
            V::Unknown(text) => {
                self.set_status(&format!("voice: didn't understand \"{text}\""), now_ms);
            }
        }
        None
    }

    /// Open `path` (same as `open_file`) and land the caret at `line` (a
    /// 0-based line index — project search's own convention, matching
    /// `Pos::line`/`goto_line`), then aim the camera at it. Used by project
    /// search's jump-to-hit; `open_file` alone would land at char 0, losing
    /// exactly the thing a search result promised.
    pub fn open_file_at_line(&mut self, path: PathBuf, line: usize, now_ms: u64) {
        if self.open_file(path, now_ms) {
            self.editor.goto_line(line);
            self.follow_caret = true;
        }
    }

    /// Caret char index for a click at content-space (x px, y px).
    ///
    /// Kept for callers that only have a content y. The view no longer uses
    /// it: dividing by `line_h` gives a row boundary half a pixel away from
    /// where `page::PageGrid` actually painted one, so a click in that band
    /// landed a row off. It goes through [`Self::caret_from_click_line`] with
    /// the grid's answer instead. Same function underneath — this wrapper is
    /// how the existing tests keep testing the thing they were written for.
    pub fn caret_from_click(&self, x: f32, y: f32, line_h: f32, char_w: f32) -> usize {
        let line = ((y / line_h).floor().max(0.0)) as usize;
        self.caret_from_click_line(x, line, char_w)
    }

    /// Caret char index for a click at content-space `x`, on an already-known
    /// `line`. The line is clamped to the buffer here so every caller gets
    /// that for free.
    pub fn caret_from_click_line(&self, x: f32, line: usize, char_w: f32) -> usize {
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

    #[test]
    fn completion_replace_start_stops_after_the_last_double_colon() {
        // The exact bug from the screenshots: typing `serde_json::` then
        // accepting `StreamDeserializer` must produce
        // `serde_json::StreamDeserializer`, not just `StreamDeserializer` --
        // which means the replace range must start right after the `::`,
        // not at the beginning of `serde_json`.
        let word = "serde_json::";
        let word_start = 4; // arbitrary — the caret column the word began at
        let replace_start = AppState::completion_replace_start(word_start, word);
        assert_eq!(
            replace_start,
            word_start + word.chars().count(),
            "with nothing typed after `::`, the replace range must be EMPTY \
             (start == caret), so accepting only inserts, never deletes \
             the already-typed module path"
        );
    }

    #[test]
    fn completion_replace_start_with_a_partial_segment_typed() {
        // serde_json::Strea| (partial segment typed) -- replace only
        // "Strea", keep "serde_json::".
        let word = "serde_json::Strea";
        let word_start = 4;
        let replace_start = AppState::completion_replace_start(word_start, word);
        let expected = word_start + "serde_json::".chars().count();
        assert_eq!(replace_start, expected, "must replace only the partial segment, not the module path");
    }

    #[test]
    fn completion_replace_start_with_a_nested_path_uses_the_last_pair() {
        // std::collections::Has| -- must stop after the LAST `::`
        // (before "Has"), not the first (which would eat "collections" too).
        let word = "std::collections::Has";
        let word_start = 0;
        let replace_start = AppState::completion_replace_start(word_start, word);
        let expected = "std::collections::".chars().count();
        assert_eq!(replace_start, expected, "must use the LAST `::`, not the first");
    }

    #[test]
    fn completion_replace_start_is_unchanged_for_a_plain_identifier() {
        // No `::` at all -- an ordinary local-symbol completion, the
        // pre-fix behavior, must be untouched.
        let word = "wor";
        let word_start = 7;
        assert_eq!(AppState::completion_replace_start(word_start, word), word_start);
    }

    #[test]
    fn completion_replace_start_empty_word_is_a_no_op() {
        assert_eq!(AppState::completion_replace_start(3, ""), 3);
    }

    fn typed(s: &mut AppState, text: &str, t: u64) {
        s.handle(Ev::Text(text.into()), t);
    }
    fn key(s: &mut AppState, k: K, ctrl: bool, t: u64) {
        s.handle(Ev::Key { key: k, ctrl, shift: false, alt: false }, t);
    }
    fn key_shift(s: &mut AppState, k: K, ctrl: bool, t: u64) {
        s.handle(Ev::Key { key: k, ctrl, shift: true, alt: false }, t);
    }

    // ── multi-cursor ────────────────────────────────────────────────────────
    //
    // These test the SHELL's half — `cursors()`/`set_cursors()`/`fan_out_*`
    // and the keys that reach them — not the arithmetic in
    // `aperture_core::multicursor`, which has its own suite.

    #[test]
    fn ctrl_d_selects_the_word_then_the_next_occurrence() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("let total = total + 1;");
        s.editor.sel = aperture_core::editor::Selection::caret(2); // inside "let"... move to "total"
        s.editor.sel = aperture_core::editor::Selection::caret(6); // inside first "total"

        key(&mut s, K::D, true, 0);
        assert!(s.extra_cursors.is_empty(), "first press only selects the word");
        assert_eq!((s.editor.sel.min(), s.editor.sel.max()), (4, 9));

        key(&mut s, K::D, true, 1);
        assert_eq!(s.extra_cursors.len(), 1, "second press adds the next occurrence");
    }

    #[test]
    fn ctrl_shift_d_is_not_swallowed_by_the_plain_ctrl_d_arm() {
        // The arm-ordering hazard, asserted rather than trusted: a bare
        // `(D, true)` placed above the shift guard compiles without a
        // warning and silently turns Ctrl+Shift+D into Ctrl+D.
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("q a q b q");
        s.editor.sel = aperture_core::editor::Selection::caret(8); // the LAST q
        key_shift(&mut s, K::D, true, 0); // promote to the word
        assert!(s.extra_cursors.is_empty(), "first press only selects");
        key_shift(&mut s, K::D, true, 1);
        assert_eq!(s.extra_cursors.len(), 1, "a second cursor appeared");
        let mins: Vec<usize> = std::iter::once(s.editor.sel.min())
            .chain(s.extra_cursors.iter().map(|c| c.min()))
            .collect();
        assert!(mins.iter().any(|m| *m == 4), "it went BACKWARD to the middle q: {mins:?}");
        assert!(!mins.iter().any(|m| *m == 0), "not forward-wrapped to the first: {mins:?}");
    }

    #[test]
    fn ctrl_u_removes_the_cursor_ctrl_d_just_added() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("let total = total + total;");
        s.editor.sel = aperture_core::editor::Selection::caret(6);
        key(&mut s, K::D, true, 0);
        key(&mut s, K::D, true, 1);
        key(&mut s, K::D, true, 2);
        assert_eq!(s.extra_cursors.len(), 2);
        key(&mut s, K::U, true, 3);
        assert_eq!(s.extra_cursors.len(), 1, "one press, one cursor gone");
        key(&mut s, K::U, true, 4);
        assert!(s.extra_cursors.is_empty());
        key(&mut s, K::U, true, 5);
        let (msg, _) = s.status.clone().expect("the no-op explains itself");
        assert!(msg.contains("only one cursor"), "{msg}");
    }

    #[test]
    fn ctrl_d_with_nothing_further_sets_a_status_and_adds_no_cursor() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("let total = 1;");
        s.editor.sel = aperture_core::editor::Selection::caret(6);
        key(&mut s, K::D, true, 0);
        key(&mut s, K::D, true, 1);
        assert!(s.extra_cursors.is_empty());
        let (msg, _) = s.status.clone().expect("a status message explains the no-op");
        assert!(msg.contains("nothing more"), "{msg}");
    }

    #[test]
    fn typing_with_multiple_cursors_inserts_at_every_one_as_one_undo_step() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("total\ntotal\n");
        s.editor.sel = aperture_core::editor::Selection::caret(0);
        s.extra_cursors = vec![aperture_core::editor::Selection::caret(6)];

        typed(&mut s, "x", 0);
        assert_eq!(s.editor.buffer.to_string(), "xtotal\nxtotal\n");
        // Both carets land right after their own inserted 'x'.
        assert_eq!(s.editor.sel, aperture_core::editor::Selection::caret(1));
        assert_eq!(s.extra_cursors, vec![aperture_core::editor::Selection::caret(8)]);

        s.editor.undo();
        assert_eq!(s.editor.buffer.to_string(), "total\ntotal\n", "one undo removes both inserts");
    }

    #[test]
    fn a_typed_run_across_multiple_cursors_undoes_as_one_gesture() {
        // Caught interactively, not by the test suite: typing "sum" over two
        // selected occurrences of "total" replaced both correctly, but Ctrl+Z
        // only removed the LAST character — three keystrokes, three separate
        // undo groups. Each `fan_out_insert` call was going through
        // `Editor::replace_many`, which never coalesces, unlike single-cursor
        // `insert_text`. This is the regression test for the fix
        // (`replace_many_coalescing`).
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("let total = 1;\nlet total = 2;\n");
        s.editor.sel = aperture_core::editor::Selection::caret(6); // inside first "total"
        key(&mut s, K::D, true, 0); // word-select
        key(&mut s, K::D, true, 10); // add the second "total"
        assert_eq!(s.extra_cursors.len(), 1);

        typed(&mut s, "s", 20);
        typed(&mut s, "u", 30);
        typed(&mut s, "m", 40);
        assert_eq!(s.editor.buffer.to_string(), "let sum = 1;\nlet sum = 2;\n");

        s.editor.undo();
        assert_eq!(s.editor.buffer.to_string(), "let total = 1;\nlet total = 2;\n",
            "one undo for the whole typed run, at both cursors");
    }

    #[test]
    fn a_motion_between_keystrokes_seals_the_group_same_as_single_cursor_typing() {
        // `Editor::set_head` (every motion funnels through it) already calls
        // `History::seal()` — true for the single-cursor path today, and
        // fanning a motion out to every cursor (`moved`) calls the same
        // `set_head` once per cursor, so the seal happens there too without
        // any multi-cursor-specific code needing to know about it.
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("aa\naa\n");
        s.editor.sel = aperture_core::editor::Selection::caret(0);
        s.extra_cursors = vec![aperture_core::editor::Selection::caret(3)];

        typed(&mut s, "x", 0);
        key(&mut s, K::Right, false, 10); // motion seals, via set_head
        typed(&mut s, "y", 20);
        assert_eq!(s.editor.buffer.to_string(), "xaya\nxaya\n");
        s.editor.undo();
        assert_eq!(s.editor.buffer.to_string(), "xaa\nxaa\n",
            "the motion sealed the group: only 'y' undoes");
        s.editor.undo();
        assert_eq!(s.editor.buffer.to_string(), "aa\naa\n");
    }

    #[test]
    fn backspace_with_multiple_cursors_removes_one_char_before_each() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("aXbb\naXbb\n");
        s.editor.sel = aperture_core::editor::Selection::caret(2); // after the X on line 1
        s.extra_cursors = vec![aperture_core::editor::Selection::caret(7)]; // after the X on line 2

        key(&mut s, K::Backspace, false, 0);
        assert_eq!(s.editor.buffer.to_string(), "abb\nabb\n");
    }

    #[test]
    fn a_multibyte_insert_lands_every_cursor_at_the_right_char_offset() {
        // The unit bug this session fixed in aperture-core, exercised through
        // the shell path that actually calls it.
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("a\na\n");
        s.editor.sel = aperture_core::editor::Selection::caret(0);
        s.extra_cursors = vec![aperture_core::editor::Selection::caret(2)];

        typed(&mut s, "é", 0);
        assert_eq!(s.editor.buffer.to_string(), "éa\néa\n");
        assert_eq!(s.editor.sel, aperture_core::editor::Selection::caret(1));
        assert_eq!(s.extra_cursors, vec![aperture_core::editor::Selection::caret(4)],
            "a byte-counting shift would have landed this cursor inside the first é");
    }

    #[test]
    fn arrow_motion_fans_out_to_every_cursor() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("abcdef\nabcdef\n");
        s.editor.sel = aperture_core::editor::Selection::caret(0);
        s.extra_cursors = vec![aperture_core::editor::Selection::caret(7)];

        key(&mut s, K::Right, false, 0);
        assert_eq!(s.editor.sel, aperture_core::editor::Selection::caret(1));
        assert_eq!(s.extra_cursors, vec![aperture_core::editor::Selection::caret(8)]);
    }

    #[test]
    fn escape_collapses_to_the_primary_and_leaves_the_buffer_untouched() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("total\ntotal\n");
        s.editor.sel = aperture_core::editor::Selection::caret(0);
        s.extra_cursors = vec![aperture_core::editor::Selection::caret(6)];

        key(&mut s, K::Escape, false, 0);
        assert!(s.extra_cursors.is_empty());
        assert_eq!(s.editor.sel, aperture_core::editor::Selection::caret(0),
            "Escape returns to the PRIMARY, not the first or the last");
        assert_eq!(s.editor.buffer.to_string(), "total\ntotal\n");
    }

    #[test]
    fn ctrl_shift_l_selects_every_occurrence_of_the_current_word() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("x = 1\ny = 2\nx = 3\n");
        s.editor.sel = aperture_core::editor::Selection::caret(0); // the first "x"
        key_shift(&mut s, K::L, true, 0);
        assert_eq!(s.extra_cursors.len(), 1, "one extra cursor: the second x");
    }

    fn key_alt(s: &mut AppState, k: K, ctrl: bool, alt: bool, t: u64) {
        s.handle(Ev::Key { key: k, ctrl, shift: false, alt }, t);
    }

    #[test]
    fn ctrl_alt_down_adds_a_cursor_one_line_below_the_lowest() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("abcdef\nabcdef\nabcdef\n");
        s.editor.sel = aperture_core::editor::Selection::caret(3); // line 0, col 3
        key_alt(&mut s, K::Down, true, true, 0);
        assert_eq!(s.extra_cursors.len(), 1);
        // The NEW cursor becomes the primary (`editor.sel`) — the viewport
        // follows the one just added, same as Ctrl+D — and the original
        // caret becomes the sole entry in `extra_cursors`.
        assert_eq!(s.editor.buffer.char_to_pos(s.editor.sel.head),
            aperture_core::buffer::Pos::new(1, 3));
        assert_eq!(s.editor.buffer.char_to_pos(s.extra_cursors[0].head),
            aperture_core::buffer::Pos::new(0, 3));

        // A second press grows from the OUTERMOST cursor (line 1), not back
        // from the primary — otherwise holding the key just toggles between
        // two lines instead of building a column.
        key_alt(&mut s, K::Down, true, true, 10);
        assert_eq!(s.extra_cursors.len(), 2);
        let mut lines: Vec<usize> = s.extra_cursors.iter()
            .map(|c| s.editor.buffer.char_to_pos(c.head).line).collect();
        lines.push(s.editor.buffer.char_to_pos(s.editor.sel.head).line);
        lines.sort();
        assert_eq!(lines, vec![0, 1, 2], "one cursor on each of three consecutive lines");
    }

    #[test]
    fn ctrl_alt_up_past_the_first_line_reports_status_and_adds_nothing() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("only one line\n");
        s.editor.sel = aperture_core::editor::Selection::caret(2);
        key_alt(&mut s, K::Up, true, true, 0);
        assert!(s.extra_cursors.is_empty());
        let (msg, _) = s.status.clone().expect("a status explains the no-op");
        assert!(msg.contains("no line"), "{msg}");
    }

    #[test]
    fn plain_ctrl_up_without_alt_is_unaffected_by_the_column_cursor_binding() {
        // Regression guard: (Up, true) now has an `if alt` guard ahead of it;
        // Ctrl+Up alone (no Alt) must fall through to the catch-all, not
        // silently start adding cursors.
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("abc\nabc\n");
        s.editor.sel = aperture_core::editor::Selection::caret(1);
        key(&mut s, K::Up, true, 0); // ctrl, no alt
        assert!(s.extra_cursors.is_empty(), "Ctrl+Up alone must not add a cursor");
    }

    #[test]
    fn undo_discards_stale_secondary_cursors_rather_than_leaving_them_dangling() {
        // The exact bug found interactively: Ctrl+Shift+L across three
        // lines, type a replacement (fans out correctly), Ctrl+Z restores
        // the buffer — but with the old code `extra_cursors` was never told
        // the buffer had changed shape, so it silently carried three now-
        // meaningless offsets into whatever the NEXT keystroke did.
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("x = 1\ny = 2\nx = 3\n");
        s.editor.sel = aperture_core::editor::Selection::caret(0);
        key_shift(&mut s, K::L, true, 0); // two cursors on "x"
        assert_eq!(s.extra_cursors.len(), 1);
        typed(&mut s, "n", 10);
        key(&mut s, K::Z, true, 20); // undo
        assert!(s.extra_cursors.is_empty(),
            "undo must drop secondary cursors, not leave stale offsets");
    }

    #[test]
    fn paste_discards_stale_secondary_cursors() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("aa\naa\n");
        s.editor.sel = aperture_core::editor::Selection::caret(0);
        s.extra_cursors = vec![aperture_core::editor::Selection::caret(3)];
        s.handle(Ev::Paste("XYZ".into()), 0);
        assert!(s.extra_cursors.is_empty());
        assert_eq!(s.editor.buffer.to_string(), "XYZaa\naa\n",
            "paste still applies at the primary");
    }

    #[test]
    fn opening_a_different_file_discards_secondary_cursors() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("aa\naa\n");
        s.extra_cursors = vec![aperture_core::editor::Selection::caret(3)];
        let dir = std::env::temp_dir().join(format!("ap_test_{}.rs", std::process::id()));
        std::fs::write(&dir, "fn f() {}\n").unwrap();
        s.open_file(dir.clone(), 0);
        assert!(s.extra_cursors.is_empty());
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn opening_find_discards_secondary_cursors() {
        // `handle_find` intercepts events before `key()`'s multi-cursor arms
        // ever run — replace_current/replace_all mutate the buffer with no
        // notion of extra_cursors, so this has to collapse on the way in.
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("aa\naa\n");
        s.extra_cursors = vec![aperture_core::editor::Selection::caret(3)];
        s.open_find();
        assert!(s.extra_cursors.is_empty());
    }

    #[test]
    fn typewriter_mode_normal_commands_discard_secondary_cursors() {
        // The modal engine predates multi-cursor and mutates Editor directly
        // (Delete/Paste/Insert/OpenLine) with no fan-out awareness.
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("aa\naa\n");
        s.editor.sel = aperture_core::editor::Selection::caret(0);
        s.extra_cursors = vec![aperture_core::editor::Selection::caret(3)];
        s.typewriter_mode = true;
        s.helix = aperture_core::helix::Mode::Normal;
        // 'x' is SelectLine now, not delete-under-caret — the comment here
        // said the latter until `x` was corrected to match Helix, and the
        // test kept passing either way because `helix_key` discards extra
        // cursors before dispatching ANY act. Still the right key to press
        // for this test's actual claim; the description of what it does was
        // just stale.
        typed(&mut s, "x", 0);
        assert!(s.extra_cursors.is_empty());
    }

    #[test]
    fn x_selects_the_whole_line_without_deleting_it() {
        // The regression that matters most about the x/d split: `x` must
        // leave the buffer untouched. Bound to Act::Delete (as it was),
        // this test's first press would have destroyed a character.
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("hello\nworld\n");
        s.editor.sel = aperture_core::editor::Selection::caret(2);
        s.typewriter_mode = true;
        s.helix = aperture_core::helix::Mode::Normal;
        typed(&mut s, "x", 0);
        assert_eq!(
            s.editor.buffer.slice_to_string(0, s.editor.buffer.len_chars()),
            "hello\nworld\n",
            "x must not modify the buffer"
        );
        assert!(!s.editor.sel.is_caret(), "x must leave a selection");
        assert_eq!(s.editor.sel.min(), 0, "selection starts at the line start");
        assert_eq!(s.editor.sel.max(), 5, "and runs to the line end");
        assert_eq!(s.helix, aperture_core::helix::Mode::Visual);
    }

    #[test]
    fn x_then_d_deletes_the_line_that_was_selected() {
        // The pair the split exists to enable: select, SEE it, then operate.
        // This is the whole argument for Helix's model over vim's `dd`.
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("hello\nworld\n");
        s.editor.sel = aperture_core::editor::Selection::caret(2);
        s.typewriter_mode = true;
        s.helix = aperture_core::helix::Mode::Normal;
        typed(&mut s, "x", 0);
        typed(&mut s, "d", 0);
        assert_eq!(
            s.editor.buffer.slice_to_string(0, s.editor.buffer.len_chars()),
            "\nworld\n",
            "d after x removes exactly what x selected"
        );
    }

    #[test]
    fn shift_i_and_shift_a_land_in_insert_at_the_line_edges() {
        // The bug this pair was fixed for: both used to resolve to a bare
        // Act::Move, so the caret went to the right place and the editor
        // stayed in Normal — the next character typed would have been
        // interpreted as a command instead of inserted.
        for (key, want_col, label) in [("I", 0usize, "line start"), ("A", 5, "line end")] {
            let mut s = AppState::new(None, false);
            s.editor = aperture_core::editor::Editor::from_str("hello\nworld\n");
            s.editor.sel = aperture_core::editor::Selection::caret(3);
            s.typewriter_mode = true;
            s.helix = aperture_core::helix::Mode::Normal;
            typed(&mut s, key, 0);
            assert_eq!(
                s.editor.sel.min(), want_col,
                "Shift-{key} should sit at the {label}"
            );
            assert_eq!(
                s.helix, aperture_core::helix::Mode::Insert,
                "Shift-{key} must ENTER Insert, not just move the caret"
            );
        }
    }

    #[test]
    fn slash_opens_the_same_find_bar_ctrl_f_does() {
        // Not a second search surface — see Act::Search's doc. If this ever
        // stops being the same `FindState`, the two will drift.
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("hello\n");
        s.typewriter_mode = true;
        s.helix = aperture_core::helix::Mode::Normal;
        assert!(s.find.is_none());
        typed(&mut s, "/", 0);
        assert!(s.find.is_some(), "/ must open the find bar");
    }

    #[test]
    fn adding_a_cursor_dismisses_a_shown_symbol_popup() {
        // The popup's accept path (`popup_accept`) calls `Editor::
        // replace_range` directly, with no notion of `extra_cursors` — if a
        // popup opened before Ctrl+D and survived, accepting it afterward
        // would mutate the buffer at the primary alone while stale
        // secondary cursors kept whatever offsets they had before.
        let mut s = AppState::new(None, false);
        s.lang = aperture_core::syntax::Language::Rust;
        s.syntax.set_language(s.lang);
        typed(&mut s, "fn compute_total() {}\nfn main() { ", 0);
        for ch in "comp".chars() {
            s.handle(Ev::Text(ch.to_string()), 100);
        }
        assert!(s.popup.is_some(), "a popup should be showing for this setup");
        key(&mut s, K::D, true, 200);
        assert!(s.popup.is_none(), "Ctrl+D must dismiss a stale popup, not just the ghost");
    }

    // ── voice commands ───────────────────────────────────────────────────
    //
    // aperture_core::voice has its own suite for the grammar itself; these
    // test the SHELL half — that each VoiceCommand actually reaches the
    // right AppState method, with the right effect on real state.

    #[test]
    fn voice_save_and_undo_redo_reach_the_real_editor_methods() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("a");
        s.editor.sel = aperture_core::editor::Selection::caret(1);
        s.editor.insert_text("b", 0);
        assert_eq!(s.editor.buffer.to_string(), "ab");
        s.run_voice_command("undo", 10);
        assert_eq!(s.editor.buffer.to_string(), "a", "voice undo must call the real Editor::undo");
        s.run_voice_command("redo", 20);
        assert_eq!(s.editor.buffer.to_string(), "ab");
    }

    #[test]
    fn voice_type_inserts_literal_text_at_the_caret() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("");
        s.run_voice_command("type hello world", 0);
        assert_eq!(s.editor.buffer.to_string(), "hello world");
    }

    #[test]
    fn voice_go_to_line_is_one_indexed_the_way_a_person_says_it() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("a\nb\nc\n");
        s.run_voice_command("go to line 2", 0);
        assert_eq!(s.editor.caret_pos().line, 1, "spoken 'line 2' is index 1");
    }

    #[test]
    fn voice_go_to_symbol_navigates_using_this_files_own_outline() {
        let mut s = AppState::new(None, false);
        s.lang = aperture_core::syntax::Language::Rust;
        s.syntax.set_language(s.lang);
        s.editor = aperture_core::editor::Editor::from_str(
            "fn compute_total() -> i32 {\n    42\n}\n\nfn main() {\n    compute_total();\n}\n",
        );
        s.run_voice_command("go to compute total", 0);
        assert_eq!(s.editor.caret_pos().line, 0, "compute_total is defined on line 0");
    }

    #[test]
    fn voice_go_to_symbol_with_no_match_falls_through_to_the_generic_unknown_message() {
        // "nothing_defined_here" never matches the vocabulary at all, so
        // parse_with_vocab returns Unknown before GoToSymbol's own
        // outline-lookup-failed message is ever reached — that message is
        // for the narrower case of a name that WAS vocab-matched but can no
        // longer be found (a stale-outline race), not "no match at all".
        let mut s = AppState::new(None, false);
        s.lang = aperture_core::syntax::Language::Rust;
        s.syntax.set_language(s.lang);
        s.editor = aperture_core::editor::Editor::from_str("fn main() {}\n");
        s.editor.sel = aperture_core::editor::Selection::caret(5);
        s.run_voice_command("go to nothing_defined_here", 0);
        assert_eq!(s.editor.caret_pos().line, 0, "no jump should have happened");
        let (msg, _) = s.status.clone().expect("a status should explain the miss");
        assert!(msg.contains("didn't understand"), "{msg}");
    }

    #[test]
    fn voice_vocabulary_grows_as_the_buffer_changes() {
        // The whole point of "buildup": a function added mid-session
        // becomes sayable without any extra step reindexing it.
        let mut s = AppState::new(None, false);
        s.lang = aperture_core::syntax::Language::Rust;
        s.syntax.set_language(s.lang);
        s.editor = aperture_core::editor::Editor::from_str("fn main() {}\n");
        assert!(s.run_voice_command("go to freshly_added", 0).is_none());
        assert!(s.status.as_ref().unwrap().0.contains("didn't understand"));

        s.editor = aperture_core::editor::Editor::from_str("fn freshly_added() {}\nfn main() {}\n");
        s.run_voice_command("go to freshly added", 0);
        assert_eq!(s.editor.caret_pos().line, 0, "the newly-typed function should now be navigable by voice");
    }

    #[test]
    fn two_speakers_sharing_a_session_each_get_their_own_again() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("a\nb\nc\nd\ne\n");
        s.run_voice_command_as("alice", "go to line 2", 0);
        s.run_voice_command_as("bob", "go to line 4", 0);

        // Bob's "again" must replay BOB's own last command (line 4), not
        // Alice's — the one multi-user behavior that's wrong by default.
        s.run_voice_command_as("bob", "again", 0);
        assert_eq!(s.editor.caret_pos().line, 3, "bob's again should return to line 4 (index 3)");

        s.run_voice_command_as("alice", "again", 0);
        assert_eq!(s.editor.caret_pos().line, 1, "alice's again should return to line 2 (index 1)");
    }

    #[test]
    fn voice_find_opens_the_bar_and_seeds_the_query() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("let total = 1;\n");
        s.run_voice_command("find total", 0);
        let f = s.find.as_ref().expect("find bar should be open");
        assert_eq!(f.query, "total");
        assert!(!f.matches.is_empty(), "the seeded query should already be matched");
    }

    #[test]
    fn voice_add_next_cursor_and_select_all_occurrences_reuse_the_real_gestures() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("x = 1\ny = 2\nx = 3\n");
        s.editor.sel = aperture_core::editor::Selection::caret(0);
        s.run_voice_command("select all occurrences", 0);
        assert_eq!(s.extra_cursors.len(), 1, "two x's total: one primary, one extra");
    }

    #[test]
    fn voice_copy_returns_clipboard_text_without_touching_the_buffer() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("hello world");
        s.editor.sel = aperture_core::editor::Selection { anchor: 0, head: 5 };
        let clip = s.run_voice_command("copy", 0);
        assert_eq!(clip, Some("hello".to_string()));
        assert_eq!(s.editor.buffer.to_string(), "hello world", "copy must not mutate the buffer");
    }

    #[test]
    fn voice_cut_returns_clipboard_text_and_removes_the_selection() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("hello world");
        s.editor.sel = aperture_core::editor::Selection { anchor: 0, head: 6 };
        let clip = s.run_voice_command("cut", 0);
        assert_eq!(clip, Some("hello ".to_string()));
        assert_eq!(s.editor.buffer.to_string(), "world");
    }

    #[test]
    fn voice_escape_clears_extra_cursors_same_as_the_keyboard() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("aa\naa\n");
        s.extra_cursors = vec![aperture_core::editor::Selection::caret(3)];
        s.run_voice_command("escape", 0);
        assert!(s.extra_cursors.is_empty());
    }

    #[test]
    fn an_unrecognized_voice_command_sets_a_status_and_touches_nothing() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("unchanged\n");
        let clip = s.run_voice_command("please refactor this", 0);
        assert_eq!(clip, None);
        assert_eq!(s.editor.buffer.to_string(), "unchanged\n");
        let (msg, _) = s.status.clone().expect("a status should explain the no-op");
        assert!(msg.contains("didn't understand"), "{msg}");
    }

    #[test]
    fn opening_voice_command_mode_discards_stale_secondary_cursors() {
        // Same class of bug as find/popup/typewriter earlier this session:
        // the palette mode change should not leave extra_cursors dangling
        // for a voice command that isn't fan-out-aware (most of them aren't).
        let mut s = AppState::new(None, false);
        s.extra_cursors = vec![aperture_core::editor::Selection::caret(0)];
        s.open_voice_command();
        assert!(s.palette.is_some());
        assert!(s.extra_cursors.is_empty(),
            "voice commands mutate through single-cursor methods; stale extras must not survive");
    }

    #[test]
    fn hover_headline_skips_a_leading_bare_crate_name_line() {
        // The exact bug found interactively: rust-analyzer's hover for a
        // function commonly starts with a bare crate-name annotation line
        // before the actual signature.
        let contents = "my_crate\n\n```rust\nfn compute_total() -> i32\n```";
        assert_eq!(AppState::hover_headline(contents), "fn compute_total() -> i32");
    }

    #[test]
    fn hover_headline_finds_a_signature_line_wherever_it_sits() {
        let contents = "some preamble\nmore preamble\nlet x: i32";
        assert_eq!(AppState::hover_headline(contents), "let x: i32");
    }

    #[test]
    fn hover_headline_falls_back_to_first_nonblank_line_with_no_signature_marker() {
        let contents = "\n  \njust some plain doc text\nmore text";
        assert_eq!(AppState::hover_headline(contents), "just some plain doc text");
    }

    #[test]
    fn hover_headline_of_empty_contents_is_an_empty_string() {
        assert_eq!(AppState::hover_headline(""), "");
        assert_eq!(AppState::hover_headline("   \n  \n"), "");
    }

    #[test]
    fn an_edit_clears_a_showing_hover_overlay() {
        // The overlay is about a caret position that no longer means
        // anything the instant the buffer under it changes — leaving it
        // showing would be describing stale code.
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("abc");
        s.hover_overlay = Some("fn foo() -> i32".to_string());
        typed(&mut s, "x", 0);
        assert!(s.hover_overlay.is_none());
    }

    #[test]
    fn escape_clears_a_showing_hover_overlay() {
        let mut s = AppState::new(None, false);
        s.hover_overlay = Some("fn foo() -> i32".to_string());
        key(&mut s, K::Escape, false, 0);
        assert!(s.hover_overlay.is_none());
    }

    #[test]
    fn a_single_cursor_never_touches_the_fan_out_path() {
        // The zero-extra-cursors case must be byte-for-byte the old behaviour
        // — this is the regression the whole feature is not allowed to cause.
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("hello");
        s.editor.sel = aperture_core::editor::Selection::caret(5);
        typed(&mut s, "!", 0);
        assert_eq!(s.editor.buffer.to_string(), "hello!");
        assert!(s.extra_cursors.is_empty());
    }

    #[test]
    fn toggle_typewriter_flips_mode_and_camera_weight() {
        let mut s = AppState::new(None, false);
        assert!(!s.typewriter_mode);
        assert_eq!(s.cam.scroll_y.rate, 12.0, "default scroll weight");
        s.toggle_typewriter(0);
        assert!(s.typewriter_mode);
        assert_eq!(s.cam.scroll_y.rate, 7.0, "on: heavier, dolly-like convergence");
        let (msg, _) = s.status.clone().unwrap();
        assert!(msg.contains("typewriter camera: on"), "{msg}");
        s.toggle_typewriter(0);
        assert!(!s.typewriter_mode);
        assert_eq!(s.cam.scroll_y.rate, 12.0, "off: restored, browsing stays snappy");
        let (msg, _) = s.status.clone().unwrap();
        assert!(msg.contains("typewriter camera: off"), "{msg}");
    }

    #[test]
    fn note_text_motion_arms_the_slide_on_insert_but_not_big_pastes() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("one\ntwo\nthree\n");
        s.editor.sel = aperture_core::editor::Selection::caret(4); // start of "two"
        s.last_len_lines = s.editor.buffer.len_lines();
        s.last_motion_version = s.editor.version;

        // Insert a line at the caret: lines from here down slide DOWN from
        // their old position (shift starts negative, decays to 0).
        typed(&mut s, "NEW\n", 0);
        s.note_text_motion();
        assert_eq!(s.line_shift.target, 0.0);
        assert_eq!(s.line_shift.pos, -1.0, "one line inserted: slide starts at -1");
        assert_eq!(s.line_shift_from, 2,
                   "post-edit caret line: only the lines that actually moved slide");

        // A large multi-line paste (>8 lines) does NOT arm the effect —
        // a page teleporting is a glitch, not motion.
        s.line_shift.snap(0.0);
        s.last_len_lines = s.editor.buffer.len_lines();
        s.last_motion_version = s.editor.version;
        s.handle(Ev::Paste("x\n".repeat(20)), 20);
        s.note_text_motion();
        assert_eq!(s.line_shift.pos, 0.0, "big paste: no slide armed");
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
        // Tab types ONE BIT, not the whole completion — that is the
        // typeout design, and this assertion predates it. `() {}` is two
        // bits: the punctuation run `()`, then ` {}` after the space.
        //
        // Updated rather than reverted, because taking a completion a
        // piece at a time is the behaviour being built: it is what lets
        // a person stop halfway through a suggestion they only half
        // agree with.
        key(&mut s, K::Tab, false, 1000);
        assert_eq!(s.editor.buffer.to_string(), "fn main()");
        key(&mut s, K::Tab, false, 1010);
        assert_eq!(s.editor.buffer.to_string(), "fn main() {}");
        // Two bits typed, so two undos to get back — each Tab is its own
        // edit and its own undo group. That is the right granularity:
        // the point of typing a completion in pieces is being able to
        // keep some of it, which means being able to undo some of it.
        key(&mut s, K::Z, true, 1100); // undo the second bit…
        assert_eq!(s.editor.buffer.to_string(), "fn main()");
        key(&mut s, K::Z, true, 1110); // …and the first
        assert_eq!(s.editor.buffer.to_string(), "fn main");
        key(&mut s, K::Z, true, 1200); // …then the typed word, as its own group
        assert_eq!(s.editor.buffer.to_string(), "fn ");
    }

    #[test]
    fn typing_through_the_ghost_keeps_it_alive() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "pr", 0);
        s.ghost = Ghost::Shown { text: "intln!".into(), generation: 0 };
        typed(&mut s, "i", 10);
        assert_eq!(s.ghost, Ghost::Shown { text: "ntln!".into(), generation: 0 });
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
    fn streamed_partial_outcomes_grow_the_ghost_text_progressively() {
        // The lower layers (aperture_complete's own engine/provider tests)
        // prove a streaming provider delivers growing partials then one
        // final outcome. This is the piece that was genuinely untested:
        // that AppState's own note_outcome, given that exact sequence,
        // actually shows the growing text rather than only reacting to
        // the final one — the behavior the code comment above claims
        // ("Ghost text updates on EVERY outcome, partial or final") but
        // nothing exercised.
        //
        // Checks the actual TEXT at each step, not just is_shown() — the
        // first version of this test only checked is_shown(), which would
        // have passed even with the real bug found while building this
        // (once Shown, on_outcome's guard silently dropped every later
        // delta for the same generation; is_shown() alone can't see that
        // the text stayed frozen).
        let mut s = AppState::new(None, false);
        typed(&mut s, "fn main", 0);
        s.ghost.request_sent(s.editor.version);
        let generation = s.editor.version;
        for partial in ["(", "() ", "() {", "() {}"] {
            s.note_outcome(&CompletionOutcome {
                generation,
                provider: "test".into(),
                latency_ms: 5,
                result: Ok(partial.to_string()),
                is_final: false,
            }, 100);
            assert_eq!(
                s.ghost.text(), Some(partial.trim()),
                "each partial should be the CURRENTLY shown text, not frozen at an earlier one"
            );
        }
        s.note_outcome(&CompletionOutcome {
            generation,
            provider: "test".into(),
            latency_ms: 20,
            result: Ok("() {}".to_string()),
            is_final: true,
        }, 200);
        assert_eq!(s.ghost.text(), Some("() {}"));
    }

    #[test]
    fn streamed_partials_do_not_flood_the_loop_pane_only_the_final_one_logs() {
        // "a 20-delta streamed completion should read as one row, not
        // twenty near-duplicates burying everything before it" — stated
        // as the reason for the `if !out.is_final { return; }` early exit,
        // but never actually driven with more than one outcome before.
        let mut s = AppState::new(None, false);
        typed(&mut s, "fn main", 0);
        s.ghost.request_sent(s.editor.version);
        let generation = s.editor.version;
        for partial in ["(", "() ", "() {", "() {}"] {
            s.note_outcome(&CompletionOutcome {
                generation,
                provider: "test".into(),
                latency_ms: 5,
                result: Ok(partial.to_string()),
                is_final: false,
            }, 100);
        }
        assert_eq!(s.loop_log.len(), 0, "no partial should have logged anything yet");
        s.note_outcome(&CompletionOutcome {
            generation,
            provider: "test".into(),
            latency_ms: 20,
            result: Ok("() {}".to_string()),
            is_final: true,
        }, 200);
        assert_eq!(s.loop_log.len(), 1, "exactly one row for the whole streamed generation");
    }

    #[test]
    fn word_accept_inserts_incrementally() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "x", 0);
        s.ghost = Ghost::Shown { text: " = compute();".into(), generation: 0 };
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
        s.ghost = Ghost::Shown { text: "zzz".into(), generation: 0 };
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
    fn a_key_that_edits_or_moves_ends_the_composition_on_screen() {
        // The screenshot: composing text left over, then Enter -- the new
        // line must not show the old composition at its caret.
        let mut s = AppState::new(None, false);
        typed(&mut s, "import (\n    \"strings\"", 0);
        s.handle(Ev::Preedit("ngs\"".into()), 10);
        assert_eq!(s.preedit, "ngs\"");
        s.handle(Ev::Key { key: K::Enter, ctrl: false, shift: false, alt: false }, 20);
        assert!(s.preedit.is_empty(), "Enter cleared the stale composition: {:?}", s.preedit);
        for key in [K::Tab, K::Backspace, K::Left] {
            s.handle(Ev::Preedit("x".into()), 30);
            s.handle(Ev::Key { key, ctrl: false, shift: false, alt: false }, 40);
            assert!(s.preedit.is_empty(), "{key:?} cleared it");
        }
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

#[cfg(test)]
mod strip_defaults_tests {
    #[test]
    fn every_pinned_default_names_a_real_command() {
        // A pinned label that does not match a command is a DEAD BUTTON:
        // it draws, it is tappable, and `run_strip_command` reports "no
        // longer available". Three of those on a fresh strip would be a
        // worse first impression than the empty bar the pins exist to
        // avoid, so the labels are checked against the palette rather
        // than trusted to stay in sync by hand.
        let known: Vec<String> = crate::palette_ui::command_list()
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        let strip = super::default_cmd_strip();
        let pinned: Vec<&str> = strip.entries().iter().map(|c| c.id.as_str()).collect();
        assert!(!pinned.is_empty(), "a fresh strip must not be empty");
        for id in pinned {
            assert!(known.iter().any(|k| k == id),
                    "pinned {id:?} is not a palette command — a dead button");
        }
    }
}


// ── navigation path + unsaved-edit guard ────────────────────────────────────
#[cfg(test)]
mod nav_tests {
    use super::*;
    fn typed(s: &mut AppState, text: &str, t: u64) {
        s.handle(Ev::Text(text.into()), t);
    }

    fn nav_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ap_nav_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();
        std::fs::write(d.join("sub/b.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
        d
    }
    fn caret_line_col(s: &AppState) -> (usize, usize) {
        let p = s.editor.buffer.char_to_pos(s.editor.sel.head);
        (p.line, p.col)
    }

    #[test]
    fn a_file_switch_never_silently_drops_unsaved_edits() {
        let d = nav_dir("dirty");
        let mut s = AppState::new(None, false);
        assert!(s.open_file(d.join("a.txt"), 0));
        typed(&mut s, "EDIT ", 1);
        assert!(s.dirty);
        assert!(!s.open_file(d.join("sub/b.txt"), 2), "refused while dirty");
        assert!(s.editor.buffer.to_string().starts_with("EDIT "), "the edit is still there");
        assert!(s.status.as_ref().unwrap().0.contains("unsaved"));
        assert!(!s.open_file(d.join("a.txt"), 3), "reopening the same file is a revert: refused too");
        assert!(s.editor.buffer.to_string().starts_with("EDIT "));
        assert!(!s.open_file(d.join("sub/b.txt"), 4), "a different request re-arms rather than confirms");
        assert!(s.open_file(d.join("sub/b.txt"), 5), "the same request twice in a row discards, on purpose");
        assert_eq!(s.path.as_deref(), Some(d.join("sub/b.txt").as_path()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn alt_left_returns_to_the_exact_spot_a_jump_started_and_alt_right_retakes_it() {
        let d = nav_dir("path");
        let mut s = AppState::new(None, false);
        s.open_file(d.join("a.txt"), 0);
        s.go_to_site(d.join("a.txt"), 2, 3, 1); // "gamma", col 3
        s.palette_jump(d.join("sub/b.txt"), Some(3), aperture_core::entity::Relation::TextMatch, 2);
        assert_eq!(s.path.as_deref(), Some(d.join("sub/b.txt").as_path()));
        assert_eq!(caret_line_col(&s).0, 3);
        assert!(s.nav.can_go_back());

        s.handle(Ev::Key { key: K::Left, ctrl: false, shift: false, alt: true }, 3);
        assert_eq!(s.path.as_deref(), Some(d.join("a.txt").as_path()), "back in the file it started in");
        assert_eq!(caret_line_col(&s), (2, 3), "at the exact line AND column");

        s.handle(Ev::Key { key: K::Right, ctrl: false, shift: false, alt: true }, 4);
        assert_eq!(s.path.as_deref(), Some(d.join("sub/b.txt").as_path()));
        assert_eq!(caret_line_col(&s).0, 3, "forward lands where the jump landed");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn alt_left_is_not_swallowed_by_the_plain_left_arm() {
        // The dispatch matches (key, ctrl); an unguarded (Left, false) above
        // the Alt arm would move the caret one char and never navigate.
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::editor::Editor::from_str("abc\n");
        s.editor.sel = aperture_core::editor::Selection::caret(2);
        s.handle(Ev::Key { key: K::Left, ctrl: false, shift: false, alt: true }, 0);
        assert_eq!(s.editor.sel.head, 2, "Alt+Left did not move the caret");
        assert!(s.status.as_ref().unwrap().0.contains("nothing to go back"));
    }

    #[test]
    fn going_back_into_another_file_is_refused_while_this_one_is_dirty_and_stays_undone() {
        let d = nav_dir("backdirty");
        let mut s = AppState::new(None, false);
        s.open_file(d.join("a.txt"), 0);
        s.palette_jump(d.join("sub/b.txt"), Some(1), aperture_core::entity::Relation::Opened, 1);
        typed(&mut s, "X", 2);
        s.nav_go(true, 3);
        assert_eq!(s.path.as_deref(), Some(d.join("sub/b.txt").as_path()), "did not leave the dirty buffer");
        assert!(s.nav.can_go_back(), "the path cursor did not move either, so Alt+Left still works after saving");
        assert!(!s.nav.can_go_forward());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_jump_inside_the_same_file_keeps_unsaved_edits_and_can_be_walked_back() {
        let d = nav_dir("same");
        let mut s = AppState::new(None, false);
        s.open_file(d.join("a.txt"), 0);
        typed(&mut s, "Z", 1); // dirty, caret line 0
        s.palette_jump(d.join("a.txt"), Some(3), aperture_core::entity::Relation::References, 2);
        assert!(s.editor.buffer.to_string().starts_with("Zalpha"), "same file: moved within the open buffer");
        assert_eq!(caret_line_col(&s).0, 3);
        s.nav_go(true, 3);
        assert_eq!(caret_line_col(&s), (0, 1), "back to just after the Z");
        let _ = std::fs::remove_dir_all(&d);
    }


    fn outcome(calls: Vec<aperture_lsp::IncomingCall>, refs: Vec<aperture_lsp::Location>) -> crate::lsp_worker::CallersOutcome {
        crate::lsp_worker::CallersOutcome { generation: 0, calls, refs, error: None }
    }
    fn uri(p: &std::path::Path) -> String {
        format!("file://{}", p.display())
    }

    #[test]
    fn one_caller_jumps_straight_there_at_the_utf16_corrected_column() {
        let d = nav_dir("callers1");
        // "é" is one char but one UTF-16 unit; "𝄞" is one char, TWO units.
        std::fs::write(d.join("sub/b.txt"), "zero\n  𝄞 target();\n").unwrap();
        let mut s = AppState::new(None, false);
        s.open_file(d.join("a.txt"), 0);
        let call = aperture_lsp::IncomingCall {
            name: "caller".into(), kind: 12, uri: uri(&d.join("sub/b.txt")),
            line: 1, character: 0, sites: vec![(1, 5)], // after "  𝄞 " = 2+2+1 units
        };
        s.show_callers(outcome(vec![call], vec![]), 1);
        assert!(s.palette.is_none(), "one answer needs no list");
        assert_eq!(s.path.as_deref(), Some(d.join("sub/b.txt").as_path()));
        assert_eq!(caret_line_col(&s), (1, 4), "UTF-16 col 5 is char col 4 on that line");
        assert_eq!(s.nav.all().last().unwrap().rel, aperture_core::entity::Relation::CalledBy);
        s.nav_go(true, 2);
        assert_eq!(s.path.as_deref(), Some(d.join("a.txt").as_path()), "Alt+Left comes back");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn several_call_sites_open_a_list_one_row_per_site() {
        let d = nav_dir("callers2");
        let mut s = AppState::new(None, false);
        s.open_file(d.join("a.txt"), 0);
        let call = aperture_lsp::IncomingCall {
            name: "caller".into(), kind: 12, uri: uri(&d.join("sub/b.txt")),
            line: 0, character: 0, sites: vec![(1, 0), (3, 1)],
        };
        s.show_callers(outcome(vec![call], vec![]), 1);
        let p = s.palette.as_ref().expect("a list");
        assert_eq!(p.mode, crate::palette_ui::Mode::Callers);
        assert!(p.callers_are_calls);
        assert_eq!(p.callers.len(), 2, "two sites, two rows");
        assert!(p.callers[1].detail.contains("sub/b.txt:4") && p.callers[1].detail.contains("four"), "{}", p.callers[1].detail);
        assert_eq!(s.path.as_deref(), Some(d.join("a.txt").as_path()), "nothing moved yet");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn references_are_the_fallback_and_are_labelled_as_mentions() {
        let d = nav_dir("callers3");
        let mut s = AppState::new(None, false);
        s.open_file(d.join("a.txt"), 0);
        let refs = vec![
            aperture_lsp::Location { uri: uri(&d.join("a.txt")), line: 2, character: 0 },
            aperture_lsp::Location { uri: uri(&d.join("sub/b.txt")), line: 0, character: 1 },
        ];
        s.show_callers(outcome(vec![], refs), 1);
        let p = s.palette.as_ref().unwrap();
        assert!(!p.callers_are_calls);
        assert_eq!(p.callers[0].title, "a.txt");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_server_without_either_says_so() {
        let mut s = AppState::new(None, false);
        let mut o = outcome(vec![], vec![]);
        o.error = Some("the language server offers neither call hierarchy nor references".into());
        s.show_callers(o, 1);
        assert!(s.status.as_ref().unwrap().0.contains("neither"));
        assert!(s.palette.is_none());
    }

    #[test]
    fn shift_f12_is_not_plain_f12() {
        let mut s = AppState::new(None, false);
        s.handle(Ev::Key { key: K::F12, ctrl: false, shift: true, alt: false }, 0);
        assert!(s.status.as_ref().unwrap().0.starts_with("callers:"), "{:?}", s.status);
    }


    /// A throwaway repo with two committed files. GIT_CONFIG_GLOBAL is the
    /// caller's business (a signing config can hang `git commit`).
    fn repo(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ap_repo_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git").arg("-C").arg(&d).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(d.join("src/a.rs"), "fn alpha() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "a one"]);
        std::fs::write(d.join("src/b.rs"), "fn beta() {}\n\nfn gamma() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "b one"]);
        std::fs::write(d.join("src/b.rs"), "fn beta() {}\n\nfn gamma() { 1; }\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "b two"]);
        d
    }

    #[test]
    fn opening_another_file_brings_its_own_repo_and_history() {
        let d = repo("hist");
        let mut s = AppState::new(None, false); // a scratch start: no repo known
        assert!(s.open_file(d.join("src/a.rs"), 0));
        assert_eq!(s.git.as_ref().map(|(_, rel)| rel.as_str()), Some("src/a.rs"));
        assert_eq!(s.revs.len(), 1, "a.rs has one commit");
        assert!(s.open_file(d.join("src/b.rs"), 1));
        assert_eq!(s.git.as_ref().map(|(_, rel)| rel.as_str()), Some("src/b.rs"), "not a.rs's");
        assert_eq!(s.revs.len(), 2, "b.rs's own two commits");
        assert_eq!(s.workspace_root().canonicalize().unwrap(), d.canonicalize().unwrap(), "the repo root, not src/");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_bucket_keeps_places_by_name_in_git_and_follows_a_moved_function() {
        use crate::palette_ui::BucketAct;
        let d = repo("bucket");
        let mut s = AppState::new(None, false);
        s.open_file(d.join("src/b.rs"), 0);
        s.go_to_site(d.join("src/b.rs"), 2, 4, 0); // inside gamma
        s.open_buckets(0);
        let rows = s.bucket_rows(None, "Collisions");
        assert_eq!(rows[0].act, BucketAct::Create("Collisions".into()));
        s.bucket_act(BucketAct::Create("Collisions".into()), 1);
        assert_eq!(s.palette.as_ref().unwrap().bucket_open, Some(0));
        s.bucket_act(BucketAct::AddHere, 2);

        // Stored as a git ref, readable by a fresh session.
        let mut fresh = AppState::new(None, false);
        fresh.open_file(d.join("src/a.rs"), 0);
        let ws = fresh.bucket_ws().clone();
        assert_eq!(ws.buckets.len(), 1);
        let e = &ws.buckets[0].entries[0];
        assert_eq!((e.path.as_str(), e.symbol.as_deref()), ("src/b.rs", Some("gamma")));

        // gamma moves down three lines; the entry still finds it.
        std::fs::write(d.join("src/b.rs"), "fn beta() {}\n\n\n\n\nfn gamma() { 1; }\n").unwrap();
        let rows = fresh.bucket_rows(Some(0), "");
        let row = rows.iter().find(|r| r.label.starts_with("gamma")).expect("the entry row");
        assert_eq!(row.act, BucketAct::Jump { path: d.join("src/b.rs"), line: 5 });
        fresh.bucket_act(row.act.clone(), 3);
        assert_eq!(fresh.path.as_deref(), Some(d.join("src/b.rs").as_path()));
        assert_eq!(caret_line_col(&fresh).0, 5);
        assert!(fresh.nav.can_go_back(), "a bucket jump is a nav step too");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_renamed_entry_is_shown_not_dropped_and_delete_asks_first() {
        use crate::palette_ui::BucketAct;
        let d = repo("rename");
        let mut s = AppState::new(None, false);
        s.open_file(d.join("src/b.rs"), 0);
        s.go_to_site(d.join("src/b.rs"), 0, 3, 0); // beta
        s.open_buckets(0);
        s.bucket_act(BucketAct::Create("tmp".into()), 1);
        s.bucket_act(BucketAct::AddHere, 2);
        std::fs::write(d.join("src/b.rs"), "fn beta_renamed() {}\n").unwrap();
        s.open_file(d.join("src/b.rs"), 3); // re-read from disk (clean buffer)
        s.palette = Some(crate::palette_ui::PaletteState::buckets(Some(0)));
        let rows = s.bucket_rows(Some(0), "");
        assert!(rows.iter().any(|r| r.label.starts_with("beta ") && r.detail.contains("not found")), "{rows:?}");
        s.bucket_act(BucketAct::DeleteBucket, 4);
        assert_eq!(s.buckets.as_ref().unwrap().buckets.len(), 1, "first press only asks");
        s.bucket_act(BucketAct::DeleteBucket, 5);
        assert!(s.buckets.as_ref().unwrap().buckets.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn ctrl_k_opens_buckets() {
        let mut s = AppState::new(None, false);
        s.handle(Ev::Key { key: K::LetterK, ctrl: true, shift: false, alt: false }, 0);
        assert_eq!(s.palette.as_ref().map(|p| p.mode.clone()), Some(crate::palette_ui::Mode::Bucket));
    }


    /// The chords the strip sends are the ones the editor binds: a strip
    /// button that sends a chord nothing handles would be a dead button.
    #[test]
    fn every_keyboard_strip_chord_reaches_a_binding() {
        for a in AppState::strip_actions() {
            let mut s = AppState::new(None, false);
            let (mut ctrl, mut shift, mut alt) = (false, false, false);
            let mut key = a.keys.as_str();
            while let Some((m, rest)) = key.split_once('+') {
                match m {
                    "Ctrl" => ctrl = true,
                    "Shift" => shift = true,
                    "Alt" => alt = true,
                    _ => panic!("modifier {m}"),
                }
                key = rest;
            }
            let k = match key {
                "Left" => K::Left, "Right" => K::Right, "F12" => K::F12,
                "t" => K::T, "k" => K::LetterK, "f" => K::F, "p" => K::P,
                other => panic!("no K for {other}"),
            };
            assert!(s.palette.is_none() && s.find.is_none() && s.status.is_none(), "a clean start");
            s.handle(Ev::Key { key: k, ctrl, shift, alt }, 0);
            let did = s.palette.is_some() || s.find.is_some() || s.status.is_some();
            assert!(did, "{} ({}) did nothing", a.label, a.keys);
        }
    }

    #[test]
    fn the_strip_is_only_sent_when_enabled_and_focus_changes() {
        let mut s = AppState::new(None, false);
        s.sync_keyboard_strip(true);
        assert_eq!(s.strip_sent, None, "tests never talk to a real keyboard");
        s.strip_enabled = true;
        s.sync_keyboard_strip(true);
        assert_eq!(s.strip_sent, Some(true));
    }

}
