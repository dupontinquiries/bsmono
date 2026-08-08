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
use aperture_complete::{make_request, CompletionOutcome, Debounce, Provider};
use aperture_core::syntax::{Language, Lexer, SyntaxCache};
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
    Space,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ev {
    Text(String),
    Key { key: K, ctrl: bool, shift: bool, alt: bool },
    Paste(String),
    Copy,
    Cut,
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
    pub syntax: SyntaxCache,

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
            syntax: SyntaxCache::new(Lexer::new(lang)),
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
        s
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

    // ── the event loop ──────────────────────────────────────────────────────

    pub fn handle(&mut self, ev: Ev, now_ms: u64) -> Effects {
        let mut fx = Effects::default();
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
        }
        fx
    }

    fn type_text(&mut self, t: &str, now_ms: u64) {
        let consumed = self.ghost.on_text_inserted(t);
        self.editor.insert_text(t, now_ms);
        self.dirty = true;
        self.syntax.invalidate_from(self.editor.caret_pos().line.saturating_sub(0));
        self.follow_caret = true;
        if consumed {
            // Ghost absorbed the keystroke: no re-request, no debounce churn.
            self.debounce.cancel();
        } else {
            self.arm_completion(now_ms);
        }
    }

    fn after_edit(&mut self, now_ms: u64) {
        self.dirty = true;
        let line = self.editor.caret_pos().line;
        self.syntax.invalidate_from(line.saturating_sub(1));
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
        // Ghost interception first: Tab / Ctrl+→ / Esc while shown.
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
                        self.syntax.invalidate_from(self.editor.caret_pos().line);
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
            (Space, true) => self.trigger_completion_now(now_ms),
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
                self.editor.backspace(now_ms);
                self.after_edit(now_ms);
                self.arm_completion(now_ms);
            }
            (Delete, _) => {
                self.ghost.dismiss();
                self.editor.delete_forward(now_ms);
                self.after_edit(now_ms);
            }
            (Enter, _) => {
                self.ghost.dismiss();
                self.editor.newline(now_ms);
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
        let line = self.editor.caret_pos().line;
        self.syntax.invalidate_from(line.saturating_sub(1));
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
        let req = make_request(&before, &after, lang, self.editor.version, PREFIX_BUDGET, SUFFIX_BUDGET);
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
        self.ghost.on_outcome(out.generation, self.editor.version, &out.result);
        if self.ghost.is_shown() && !was_shown {
            self.ghost_since = Some(now_ms);
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
        self.palette = Some(crate::palette_ui::PaletteState::open_file());
        self.ghost.dismiss();
        self.debounce.cancel();
    }

    /// Caret char index for a click at content-space (x px, y px).
    pub fn caret_from_click(&self, x: f32, y: f32, line_h: f32, char_w: f32) -> usize {
        let line = ((y / line_h).floor().max(0.0)) as usize;
        let line = line.min(self.editor.buffer.len_lines().saturating_sub(1));
        let col = ((x / char_w) + 0.5).floor().max(0.0) as usize;
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
}
