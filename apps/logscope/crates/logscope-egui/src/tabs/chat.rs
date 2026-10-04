//! Chat tab — the message + context system.
//!
//! Architecture:
//!  - **Messages**: the conversation, a flat list of user/assistant turns.
//!  - **Context**: an editable system prompt that always leads the prompt (the
//!    "context" the model is given), separate from the turns.
//!  - **Threading**: a toggle. When on, prior turns are threaded into the prompt
//!    (multi-turn conversation); when off, each message is single-shot (context +
//!    just the current message), which is handy for clean speed tests.
//!  - **Composer**: a dedicated input box (Enter sends, Shift+Enter newline) with
//!    Send / Stop, plus live tok/s.
//!
//! The prompt is assembled per backend: the Llama-3 instruct template (special
//! tokens) for the clean-room wgpu engine, the plain transcript for llama.cpp.
use crate::backend::{BackendKind, Evt, Inference, TemplateKind};
use crate::model::ModelState;
use crate::theme;
use egui::RichText;
use logscope::llm_backend::GenParams;
use std::time::Instant;

// The slab's geometry and animation helpers live in `chat_slab.rs`. Glob-imported so the call
// sites below read exactly as they did — the split is about where the code lives, not about
// renaming everything that uses it.
#[path = "chat_slab.rs"]
mod chat_slab;
use chat_slab::*;

// The autosave's on-disk shape lives in `chat_save.rs` — the serialised structs and the two
// conversions, which are what you read together when the format changes.
#[path = "chat_save.rs"]
mod chat_save;
use chat_save::*;

// The transcript's formatting helpers live in `chat_fmt.rs` — timestamps, ages, label
// wrapping, progress summaries. Glob-imported so the call sites read as they did.
#[path = "chat_fmt.rs"]
mod chat_fmt;
use chat_fmt::*;

// The physical dock's panel and control mapping live in `chat_dock.rs` — the four pieces that
// have to agree about what each knob is.
#[path = "chat_dock.rs"]
mod chat_dock;
use chat_dock::*;

// `Chat::ui`, the tab's top-level draw, lives in `chat_draw.rs`.
#[path = "chat_draw.rs"]
mod chat_draw;

// `Chat::spatial_ui` lives in `chat_spatial.rs` — the biggest method here by a wide margin.
// A child module, because it needs `Chat`'s private fields.
#[path = "chat_spatial.rs"]
mod chat_spatial;



// ── PHYSICAL CONTROL DOCK ────────────────────────────────────────────────────────────────────────
// The camera controls (Overview / Tour / Freecam) rendered as IN-SCENE physical buttons — real 3D
// objects you press — instead of a 2D toolbar, driven by the tested `physui` FSM. IDs are stable so
// the panel, the projected hit-rects, and the action mapping all refer to the same control.
const DOCK_OVERVIEW: usize = 601; // Momentary — recenter + fit the whole graph
const DOCK_TOUR: usize = 602;     // Toggle    — fly through the palace / stop
const DOCK_FREECAM: usize = 603;  // Toggle    — free-flight camera on / off
const DOCK_PRUNE: usize = 604;    // Momentary — prune superseded turns (churn) to free context
const DOCK_UNPRUNE: usize = 606;  // Momentary — restore every pruned turn to context
const DOCK_IDS: [usize; 5] = [DOCK_OVERVIEW, DOCK_TOUR, DOCK_FREECAM, DOCK_PRUNE, DOCK_UNPRUNE];

// The Context/params panel, ported to physical DIALS you drag — each bound to a `self.ctx` field. (The time
// dial was removed: the scene tracks system time.) IDs, ranges, short labels, and stack order in one place.
const PARAM_TEMP: usize = 610;   // temperature
const PARAM_NCTX: usize = 611;   // ctx window (context length)
const PARAM_MAXTOK: usize = 612; // reply budget
const PARAM_REPEAT: usize = 613; // repeat penalty
const PARAM_MINP: usize = 614;   // min-p
/// (dial id, short label, min, max, integer?) — order = top→bottom on the physical settings panel.
const PARAM_DIALS: [(usize, &str, f32, f32, bool); 5] = [
    (PARAM_TEMP,   "TEMP",  0.0,   1.5,     false),
    (PARAM_NCTX,   "CTX",   512.0, 32768.0, true),
    (PARAM_MAXTOK, "REPLY", 64.0,  4096.0,  true),
    (PARAM_REPEAT, "REP",   1.0,   1.5,     false),
    (PARAM_MINP,   "MINP",  0.0,   0.3,     false),
];

// The right-sidebar popout's remaining controls (backend, persona, model), ported to physical SELECTORS
// beside the dials — a press cycles to the next option, wrapping (physui::ControlKind::Selector). MODEL_SEL's
// option count is discovered at runtime (the Ollama/local model list), so it's registered with a placeholder
// count of 1 and corrected every frame via `Panel::set_selector_count` once the real list is known.
const BACKEND_SEL: usize = 620; // LlamaCpp / Wgpu
const PERSONA_SEL: usize = 621; // PERSONAS table
const MODEL_SEL: usize = 622;   // ModelState::items — dynamic count
/// Which job the selected model does: Liaison / Disciple / Thinker (see logscope::modelroles).
const ROLE_SEL: usize = 623;





#[derive(Clone, Copy, PartialEq, Debug)]
enum Role { User, Assistant }





/// Register a billboarded HUD panel with `hud_pack` and return its (possibly nudged) world centre.
/// `world_c`/`world_w`/`world_h` are the panel's UNRESOLVED position and size, exactly as it would
/// have rendered before this existed; this projects that footprint to screen pixels, lets `hud_pack`
/// resolve it against everything already placed this frame, then un-projects the result back onto
/// the same `right`/`up` plane the panel already lives on. Falls back to `world_c` unprojected/off-
/// screen (same failure mode `screen_to_world` already has) rather than panicking.
#[allow(clippy::too_many_arguments)]


/// Word-wrap `text` into at most `max_lines` lines of ≤`cols` chars, for physical `label_cells` panels
/// (which render one line at a time). Wraps on whitespace; hard-splits words longer than `cols`; collapses
/// internal newlines/spaces; appends `…` to the last line when the text was truncated. Pure + testable.
/// How many wrapped lines the physical-UI reading plaque shows at once — the rest is reached by
/// scrolling (jk / impakt-cam glide) rather than being truncated with a trailing "…".
const PLAQUE_WINDOW: usize = 4;



/// A translucent "frosted glass" frame for chrome (top control bar, bottom read-out) floating OVER the
/// full-bleed scene — a dark semi-transparent fill + a faint light rim so it reads as a panel of smoked
/// glass on top of the 3D view. (egui can't sample a true backdrop blur; this is the "for now" glass.)
fn glass_frame() -> egui::Frame {
    egui::Frame {
        fill: egui::Color32::from_rgba_unmultiplied(10, 13, 20, 190),
        corner_radius: theme::r(10),
        inner_margin: egui::Margin::symmetric(10, 6),
        stroke: egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 24)),
        ..Default::default()
    }
}
/// A turn. `variants` lets an assistant turn hold multiple completions (reroll / branch); `cur`
/// selects which is shown and which streaming appends to. User turns carry a single variant plus
/// inline-edit state.
/// `model` records WHICH model produced an assistant turn. None for user turns and for anything
/// from before this was recorded — see convoprov, which skips unknown provenance rather than
/// assuming it matches the neighbours, because that is exactly what an older conversation looks
/// like and guessing there would label it with whatever happens to be loaded now.
/// `born` is an `Instant` — monotonic, so it can measure "3s ago" and can
/// NEITHER produce a wall-clock time nor survive a save. `at` is the wall clock,
/// in unix seconds, and it is why the exported transcript can say when anything
/// happened. A session reopened tomorrow used to look identical to one from a
/// minute ago.
struct Msg { role: Role, variants: Vec<String>, cur: usize, born: std::time::Instant, at: u64, editing: bool, edit_buf: String, think_open: bool, parent: Option<usize>, model: Option<String> }



impl Msg {
    fn user(text: String) -> Self { Self { role: Role::User, variants: vec![text], cur: 0, born: Instant::now(), at: wall_now(), editing: false, edit_buf: String::new(), think_open: true, parent: None, model: None } }
    fn assistant() -> Self { Self { role: Role::Assistant, variants: vec![String::new()], cur: 0, born: Instant::now(), at: wall_now(), editing: false, edit_buf: String::new(), think_open: true, parent: None, model: None } }
    fn text(&self) -> &str { &self.variants[self.cur] }
    /// All variants, for an export that must not drop regenerations.
    fn all_variants(&self) -> &[String] { &self.variants }
    fn cur_mut(&mut self) -> &mut String { &mut self.variants[self.cur] }
}
impl logscope::branchtree::Node for Msg {
    fn parent(&self) -> Option<usize> { self.parent }
}

/// A structural edit requested from a message's controls, applied after the render pass (so the
/// render can borrow `msgs` immutably-ish while the buttons just record intent).
enum MsgAction { Copy(usize), EditStart(usize), EditSave(usize), EditCancel(usize), Regenerate(usize), Delete(usize), PrevVar(usize), NextVar(usize) }
struct Stream { idx: usize, started: Instant, tokens: usize,
    /// Live progress summary shown as a temporary status while the turn generates: the latest meaningful
    /// line pulled from the streaming tail, refreshed on an interval so a long/quiet turn (a coordinator
    /// "add more blocks" run that thinks for a while before emitting visible text) still shows motion.
    /// `(text, last_refresh)`. Cheap heuristic — no second model — but structured so a small summarizer
    /// model can replace `summarize_progress` later without touching the render path.
    progress: Option<(String, Instant)>,
}
/// A background `cargo check` of the latest generated files (the sandbox oracle), run off-thread so
/// the UI stays responsive. `result` = (passed, compiler_output, sandboxed).
struct VerifyJob {
    rx: std::sync::mpsc::Receiver<(bool, String, bool)>,
    started: Instant,
    result: Option<(bool, String, bool)>,
}

/// Replicator mode's UI-side state: the columns, who is paired with whom, and where the composer
/// is aimed. The DECISIONS live in logscope::replicator (pure, tested); this holds only what a
/// frame needs to draw and what the operator has clicked.
struct ReplicatorMode {
    workers: Vec<logscope::replicator::Replicant>,
    /// Directed dilations: who sees whose latest, and how often.
    grid: logscope::replicator::ShareGrid,
    /// None = the composer broadcasts; Some(id) = it is aimed at one column.
    aim: Option<usize>,
    /// The slider's live value and the hardware bound it must respect.
    want: usize,
    slots: logscope::replicator::Slots,
    /// Per worker: its transcript (operator prompt then reply, newest last) and its own turn
    /// count. The turn count is per worker because dilation is anchored per RECEIVER — an idle
    /// column must not have its cadence advanced by a busy one.
    lanes: Vec<ReplicaLane>,
    /// What actually crossed, for the coloured edges between columns.
    crossings: logscope::replicator::CrossingLog,
    /// Open the dilation matrix under the columns.
    show_matrix: bool,
}

#[derive(Default, Clone)]
struct ReplicaLane {
    assignment: String,
    /// Role-tagged so the panel never has to infer a speaker from an index (see
    /// `replicatorlayout::LaneLine` — a lane whose reply never arrived would flip every
    /// following attribution).
    lines: Vec<crate::replicatorlayout::LaneLine>,
    turn: usize,
    /// True while the last line is a reply still streaming — so a second token stream cannot be
    /// appended onto a finished reply, and `last` (what partners quote) is never a half sentence
    /// from two different turns glued together.
    streaming: bool,
}

impl ReplicaLane {
    fn append_token(&mut self, t: &str) {
        if !self.streaming {
            self.lines.push(crate::replicatorlayout::LaneLine::worker(""));
            self.streaming = true;
        }
        if let Some(last) = self.lines.last_mut() { last.text.push_str(t); }
    }

    fn finish_reply(&mut self, full: &str) {
        let text = full.trim().to_string();
        if self.streaming { if let Some(last) = self.lines.last_mut() { last.text = text; } }
        else { self.lines.push(crate::replicatorlayout::LaneLine::worker(text)); }
        self.streaming = false;
    }

    /// What this worker last SAID — its own most recent reply, never the operator's prompt to it.
    /// `peer_context` labels this "[w2 last said]" and puts it in a partner's prompt, so handing
    /// it the operator's broadcast would quote the operator back as if a worker had written it.
    fn last_said(&self) -> Option<String> {
        self.lines.iter().rev().find(|l| !l.from_operator).map(|l| l.text.clone())
    }
}

impl ReplicatorMode {
    fn new(s: &logscope::replicator::Slots, n: usize) -> Self {
        let mut m = Self { workers: Vec::new(), grid: logscope::replicator::ShareGrid::new(0),
                           aim: None, want: n, slots: s.clone(), lanes: Vec::new(),
                           crossings: logscope::replicator::CrossingLog::new(256),
                           show_matrix: false };
        m.resize(n);
        m
    }

    /// Grow or shrink the column set, placing each worker on a host with room. Shrinking DROPS the
    /// highest ids and unpairs them — a closed column that stayed in a pair group would keep
    /// injecting a dead worker's last message into a live one's context.
    fn resize(&mut self, n: usize) {
        let n = n.clamp(self.slots.min, self.slots.selectable_max());
        while self.workers.len() > n { self.workers.pop(); self.lanes.pop(); }
        while self.workers.len() < n {
            let id = self.workers.len();
            // Fill each host to its own capacity before moving on, so columns land where the
            // memory actually is rather than round-robin onto a machine that cannot hold them.
            let mut placed = String::from("local");
            let mut seen = 0usize;
            for (host, cap) in &self.slots.per_host {
                if id < seen + cap { placed = host.clone(); break; }
                seen += cap;
            }
            self.workers.push(logscope::replicator::Replicant {
                id, host: placed, assignment: String::new() });
        }
        // The grid follows the column set: resize drops a closed worker's row AND column, so a
        // dead column cannot keep injecting its last message into a live one's context.
        self.grid.fit_to(self.workers.len());
        self.lanes.resize(self.workers.len(), ReplicaLane::default());
        self.want = n;
    }

    /// Capture for the autosave. Assignments and turns come from the lanes, which are the live
    /// copies — the workers' own `assignment` field is a per-frame mirror.
    fn snapshot(&self) -> logscope::replicator::ReplicatorSave {
        let names: Vec<String> = self.lanes.iter().map(|l| l.assignment.clone()).collect();
        let turns: Vec<u32> = self.lanes.iter().map(|l| l.turn as u32).collect();
        logscope::replicator::ReplicatorSave::capture(&self.grid, &names, &turns)
    }

    /// Reopen a saved fan at `n` columns. Transcripts are NOT restored — the chat's own autosave
    /// owns message history, and a second copy here would let the two disagree about what a worker
    /// said, which is exactly the string partners get quoted.
    fn restore(&mut self, save: &logscope::replicator::ReplicatorSave, n: usize) {
        self.resize(n);
        for i in 0..self.lanes.len() {
            let (assignment, turn) = save.worker_at(i);
            self.lanes[i].assignment = assignment.clone();
            self.lanes[i].turn = turn as usize;
            if let Some(w) = self.workers.get_mut(i) { w.assignment = assignment; }
        }
        if let Some(g) = save.restore(self.lanes.len()) { self.grid = g; }
    }

    /// Compose one operator message into per-worker prompts, record the crossings, and advance
    /// each receiving worker's turn.
    ///
    /// The prompts are RETURNED rather than sent: generation belongs to the backend path, and a
    /// mode that both decides and dispatches is a mode that cannot be tested without a GPU. The
    /// caller feeds these to the same lane machinery a normal turn uses.
    fn submit(&mut self, text: &str) -> Vec<logscope::replicator::Dispatchable> {
        let aim = self.aim;
        self.submit_to(text, aim)
    }

    /// As `submit`, with the aim supplied explicitly — Ctrl+Enter broadcasts one message without
    /// clearing the standing aim, so the field stays the operator's setting rather than something
    /// a keystroke silently rewrote.
    fn submit_to(&mut self, text: &str, aim: Option<usize>) -> Vec<logscope::replicator::Dispatchable> {
        for (i, w) in self.workers.iter().enumerate() {
            if let Some(l) = self.lanes.get_mut(i) { l.assignment = w.assignment.clone(); }
        }
        let aim = match aim { Some(i) => logscope::replicator::Aim::One(i),
                              None => logscope::replicator::Aim::All };
        let lanes = self.lanes.clone();
        let out = logscope::replicator::compose_turn(
            &self.workers, &self.grid, &aim, text,
            |id| lanes.get(id).map(|l| l.turn).unwrap_or(0),
            |id| lanes.get(id).and_then(|l| l.last_said()),
            &mut self.crossings);
        for d in &out {
            if let Some(l) = self.lanes.get_mut(d.to) {
                l.lines.push(crate::replicatorlayout::LaneLine::operator(text.trim()));
                l.turn += 1;
            }
        }
        out
    }
}

/// A running `:image` generation. sd-cli takes seconds to tens of seconds for an 8-step 1024²,
/// so it runs on a thread and the UI polls — blocking the frame would freeze the whole app on a
/// task whose whole point is that you keep working while it renders.
struct ImageJob {
    rx: std::sync::mpsc::Receiver<Result<std::path::PathBuf, String>>,
    prompt: String,
    started: Instant,
    result: Option<Result<std::path::PathBuf, String>>,
    /// The decoded PNG, uploaded once. Kept on the job rather than re-decoded per frame: a 1024²
    /// PNG is ~4 MB of RGBA and 60 decodes a second is not a thing to do by accident.
    tex: Option<egui::TextureHandle>,
    /// Set when decoding failed, so the path is still shown with a reason instead of the row
    /// silently rendering nothing — the file exists either way and the operator can open it.
    tex_err: Option<String>,
}

// ── Persistence: checkpoints, rollback, crash recovery ───────────────────────────────────────
/// On-disk form of a turn (no UI/Instant state). Role 0 = user, 1 = assistant.
/// The message list, as the view model sees it.
///
/// Implemented on a slice rather than on ChatTab so building rows borrows only the messages — the
/// renderer no longer needs `&mut self` while it paints, which is what forced the collect-then-apply
/// dance and the alternative of cloning every message to read it.
struct MsgSlice<'a>(&'a [Msg]);

impl logscope::msgview::MessageStore for MsgSlice<'_> {
    fn len(&self) -> usize { self.0.len() }
    fn is_user(&self, i: usize) -> bool { self.0[i].role == Role::User }
    fn line_count(&self, i: usize) -> usize { self.0[i].text().lines().count() }
    fn summary(&self, i: usize) -> String {
        // Only called for rows that will actually collapse, so the parse cost is paid once per visible
        // collapsed row rather than once per message per frame.
        let t = self.0[i].text();
        let edits = logscope::liaison_relay::parse_edits(t);
        let cn = logscope::liaison_relay::cliffsnotes(t, &edits, None);
        format!("\u{25b8} {}  ({} lines)", cn.label(), t.lines().count())
    }
}

fn now_unix() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

/// Build a fenced attachment block from a file's bytes for the composer. Bounded (text only). Three things
/// the old inline versions got wrong, which is what mangled pasted-in code:
///   1. FENCE SAFETY — if the file itself contains a ``` run, a 3-backtick fence around it breaks the block
///      (the file's fence closes ours early, and everything after leaks out as prose). We pick a fence of
///      enough backticks to always exceed the longest run inside the file.
///   2. LARGER CAP — 16 KB truncated real source files mid-function. Default 256 KB (LOGSCOPE_ATTACH_KB to
///      override), enough for almost any single source file.
///   3. HEAD+TAIL on LINE BOUNDARIES — for files past the cap, keep the head AND the tail (a file's end
///      usually carries the code you're actually asking about, which a head-only cut drops), cut on newlines
///      so no line is sliced mid-token, and mark the omitted middle with an exact line count.
fn fenced_attachment(name: &str, lang: &str, bytes: &[u8]) -> String {
    let cap = std::env::var("LOGSCOPE_ATTACH_KB").ok().and_then(|s| s.parse::<usize>().ok()).unwrap_or(256) * 1024;
    let text = String::from_utf8_lossy(bytes);
    let body = if text.len() <= cap {
        text.into_owned()
    } else {
        let head_budget = cap * 65 / 100;
        let tail_budget = cap - head_budget;
        let head_end = text[..head_budget.min(text.len())].rfind('\n').unwrap_or(head_budget.min(text.len()));
        let tail_from = text.len().saturating_sub(tail_budget);
        let tail_start = text[tail_from..].find('\n').map(|i| tail_from + i + 1).unwrap_or(tail_from);
        let omitted = if tail_start > head_end { text[head_end..tail_start].lines().count() } else { 0 };
        format!("{}\n\n… {omitted} lines omitted ({} KB total; showing head+tail — raise LOGSCOPE_ATTACH_KB) …\n\n{}",
            &text[..head_end], bytes.len() / 1024, &text[tail_start..])
    };
    // Fence must be longer than any backtick run inside the body, so embedded ``` can't close it early.
    let max_run = body.as_bytes().split(|&b| b != b'`').map(<[u8]>::len).max().unwrap_or(0);
    let fence = "`".repeat(max_run.max(2) + 1);
    let tag = if lang.is_empty() { name.to_string() } else { format!("{lang} {name}") };
    format!("\n{fence}{tag}\n{body}\n{fence}\n")
}

/// Content-driven COLOUR SALIENCE in [0,1]: how much the conversation is *about* colour. Mixes raw frequency
/// (saturating) with term-frequency percentage over a small colour lexicon (basic NLP: lowercase tokenise +
/// stem match). Drives scene colour that stays muted until colour is discussed, intensifying as the topic grows.
fn chat_color_salience<'a>(texts: impl Iterator<Item = &'a str>) -> f32 {
    const STEMS: [&str; 6] = ["color", "colour", "hue", "palette", "pigment", "chromatic"];
    let (mut raw, mut total) = (0u32, 0u32);
    for t in texts {
        for tok in t.split(|ch: char| !ch.is_alphanumeric()) {
            if tok.is_empty() { continue; }
            total += 1;
            let lw = tok.to_ascii_lowercase();
            if STEMS.iter().any(|s| lw.starts_with(s)) { raw += 1; } // color/colors/colored/colourful/hue…
        }
    }
    if total == 0 { return 0.0; }
    let raw_sig = 1.0 - (-(raw as f32) / 5.0).exp();   // saturating in raw count (≈0.63 at 5, 0.86 at 10)
    let tf = raw as f32 / total as f32;                // term-frequency fraction
    let tf_sig = (tf * 120.0).min(1.0);                // colour words are rare; a small TF is a strong signal
    (0.5 * raw_sig + 0.5 * tf_sig).clamp(0.0, 1.0)
}
fn shellexpand_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") { if let Ok(h) = std::env::var("HOME") { return format!("{h}/{rest}"); } }
    path.to_string()
}
fn autosave_path() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(|h| std::path::PathBuf::from(h).join(".local/share/logscope").join("chat_autosave.json"))
}
/// Maximum checkpoints kept (oldest auto-checkpoints are pruned). Bounds the autosave size.
const MAX_CHECKPOINTS: usize = 30;

struct Ctx { n_ctx: u32, max_tokens: u32, temperature: f32, repeat_penalty: f32, min_p: f32, history_turns: usize }
impl Default for Ctx {
    fn default() -> Self { Self { n_ctx: 4096, max_tokens: 512, temperature: 0.35, repeat_penalty: 1.15, min_p: 0.05, history_turns: 8 } }
}

fn est(s: &str) -> usize { s.len() / 4 + 1 } // rough tokens; the backend tokenizes exactly

/// How many times a single reply may auto-continue past the token cap before it waits for a manual
/// nudge — bounds runaway generation while letting a long file/answer finish on its own.
const MAX_AUTO_CONTINUE: u32 = 4;

/// A rendered chunk of a message: main prose, a fenced code block, or a `<think>` reasoning
/// block. Three-way styling so a coding/reasoning assistant's output reads right: thinking is
/// set apart (dimmed, collapsible), code is monospace + copyable, prose is the answer. Handles
/// an unclosed code fence OR an unclosed `<think>` mid-stream so both format live as they stream.
enum Seg { Prose(String), Code { lang: String, body: String }, Think(String) }

fn segment_message(text: &str) -> Vec<Seg> {
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        match rest.find("<think>") {
            Some(start) => {
                segment_codeprose(&rest[..start], &mut out);
                let after = &rest[start + "<think>".len()..];
                match after.find("</think>") {
                    Some(end) => {
                        let t = after[..end].trim();
                        if !t.is_empty() { out.push(Seg::Think(t.to_string())); }
                        rest = &after[end + "</think>".len()..];
                    }
                    None => { // unclosed (streaming): the tail is the thinking block
                        let t = after.trim();
                        if !t.is_empty() { out.push(Seg::Think(t.to_string())); }
                        break;
                    }
                }
            }
            None => { segment_codeprose(rest, &mut out); break; }
        }
        if rest.is_empty() { break; }
    }
    out
}

/// Split a non-thinking span into prose + fenced code blocks. Tracks fence LENGTH (CommonMark-style): a
/// block opened with N backticks stays open until a line of ≥N backticks with nothing after it but
/// whitespace. This fixes the mangling from the old naive toggle, where any ``` line flipped state — so an
/// info-string fence like ```diff, or an unbalanced fence, desynced prose/code for the rest of the message.
/// Now an info-string fence (```diff) can only OPEN, only a bare fence CLOSES, and a longer outer fence can
/// wrap inner ``` runs without being closed early.
fn segment_codeprose(text: &str, out: &mut Vec<Seg>) {
    let (mut prose, mut code, mut lang) = (String::new(), String::new(), String::new());
    let mut fence_len = 0usize; // 0 = in prose; otherwise the backtick count that opened the current block
    for line in text.split('\n') {
        let trimmed = line.trim_start();
        let ticks = trimmed.bytes().take_while(|&b| b == b'`').count();
        if fence_len == 0 {
            if ticks >= 3 { // opening fence (optionally with an info string / lang)
                if !prose.trim().is_empty() { out.push(Seg::Prose(prose.trim_end().to_string())); }
                prose.clear();
                lang = trimmed[ticks..].trim().to_string();
                fence_len = ticks;
            } else { prose.push_str(line); prose.push('\n'); }
        } else if ticks >= fence_len && trimmed[ticks..].trim().is_empty() { // bare closing fence of ≥ opener length
            out.push(Seg::Code { lang: std::mem::take(&mut lang), body: code.trim_end_matches('\n').to_string() });
            code.clear(); fence_len = 0;
        } else { code.push_str(line); code.push('\n'); }
    }
    if fence_len > 0 { out.push(Seg::Code { lang, body: code.trim_end_matches('\n').to_string() }); }
    else if !prose.trim().is_empty() { out.push(Seg::Prose(prose.trim_end().to_string())); }
}

// ── lightweight markdown for chat prose ─────────────────────────────────────────────
// Parity with logscope's GTK ui/markdown.rs (minus fenced code, which is handled by Seg::Code):
// headers, **bold**, *italic*/_italic_, `inline code`, bullet lists, and links rendered as
// styled text. Hand-rolled (no new dep). egui has no bold *weight* for the default fonts, so
// "bold" is rendered with the visuals' strong color — the same convention RichText::strong uses.
struct Run { text: String, bold: bool, italic: bool, code: bool, link: bool }

fn parse_inline(s: &str) -> Vec<Run> {
    let ch: Vec<char> = s.chars().collect();
    let mut runs: Vec<Run> = Vec::new();
    let (mut i, mut bold, mut italic) = (0usize, false, false);
    let mut cur = String::new();
    macro_rules! flush { () => {
        if !cur.is_empty() { runs.push(Run { text: std::mem::take(&mut cur), bold, italic, code: false, link: false }); }
    }; }
    while i < ch.len() {
        let c = ch[i];
        if c == '`' { // inline code `…`
            if let Some(rel) = ch[i + 1..].iter().position(|&x| x == '`') {
                flush!();
                runs.push(Run { text: ch[i + 1..i + 1 + rel].iter().collect(), bold: false, italic: false, code: true, link: false });
                i += rel + 2; continue;
            }
        }
        if c == '[' { // link [text](url) — render the display text styled, target dropped
            if let Some(rb) = ch[i..].iter().position(|&x| x == ']') {
                if i + rb + 1 < ch.len() && ch[i + rb + 1] == '(' {
                    if let Some(rp) = ch[i + rb + 1..].iter().position(|&x| x == ')') {
                        flush!();
                        runs.push(Run { text: ch[i + 1..i + rb].iter().collect(), bold, italic, code: false, link: true });
                        i += rb + 1 + rp + 1; continue;
                    }
                }
            }
        }
        if c == '*' && i + 1 < ch.len() && ch[i + 1] == '*' { flush!(); bold = !bold; i += 2; continue; }
        if c == '*' || c == '_' { flush!(); italic = !italic; i += 1; continue; }
        cur.push(c); i += 1;
    }
    flush!();
    runs
}

fn md_line_job(ui: &egui::Ui, line: &str, size: f32, base: egui::Color32) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    use egui::{FontFamily, FontId};
    // CONTRAST-ENSURED, like the accent two lines down. This was
    // `ui.visuals().strong_text_color()` — egui's untreated default — so bold runs in a message could
    // fail to read on themes where every other markdown colour had been adjusted. The fix that was
    // applied to links and inline code was never applied here.
    let strong = theme::md_strong();
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    // inline code/links use the CONTRAST-ENSURED accent: the raw palette cyan is neon on several
    // themes and simply fades against pale panels (the Cloudscape-only-legible bug) — md_accent
    // keeps the hue but nudges its value until it actually reads on the active background.
    let accent = theme::md_accent();
    for r in parse_inline(line) {
        let font = FontId::new(size, if r.code { FontFamily::Monospace } else { FontFamily::Proportional });
        let color = if r.code || r.link { accent } else if r.bold { strong } else { base };
        let mut fmt = TextFormat { font_id: font, color, ..Default::default() };
        fmt.italics = r.italic;
        if r.link { fmt.underline = egui::Stroke::new(1.0_f32, accent); }
        job.append(&r.text, 0.0, fmt);
    }
    job
}

/// Render a prose segment with inline markdown. `tail` is the streaming cursor (▌), placed on the last line.
fn render_markdown(ui: &mut egui::Ui, text: &str, tail: &str) {
    let base = ui.style().text_styles.get(&egui::TextStyle::Body).map(|f| f.size).unwrap_or(14.0);
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len().saturating_sub(1);
    for (k, raw) in lines.iter().enumerate() {
        let suffix = if k == last { tail } else { "" };
        let t = raw.trim_end();
        let trimmed = t.trim_start();
        // ATX header: 1–6 leading '#' then a space
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            let body = trimmed[hashes..].trim_start();
            let size = match hashes { 1 => base + 6.0, 2 => base + 3.0, 3 => base + 1.0, _ => base };
            ui.label(RichText::new(format!("{body}{suffix}")).size(size).strong().color(theme::md_ink()));
            continue;
        }
        // blockquote: '>' prefixed — a left accent bar + dimmed (but contrast-ensured) text on a
        // faint wash, the Cloudscape look generalized to every palette
        if let Some(rest) = trimmed.strip_prefix("> ").or_else(|| trimmed.strip_prefix(">")) {
            let bar = theme::md_quote_accent();
            let text_col = theme::md_dim();
            let resp = ui.horizontal_wrapped(|ui| {
                ui.add_space(10.0);
                ui.label(md_line_job(ui, &format!("{rest}{suffix}"), base, text_col));
            }).response;
            let r = resp.rect;
            ui.painter().rect_filled(
                egui::Rect::from_min_max(egui::pos2(r.left(), r.top()), egui::pos2(r.right(), r.bottom())),
                2.0,
                bar.gamma_multiply(0.07),
            );
            ui.painter().line_segment(
                [egui::pos2(r.left() + 3.0, r.top() + 1.0), egui::pos2(r.left() + 3.0, r.bottom() - 1.0)],
                egui::Stroke::new(2.5_f32, bar),
            );
            continue;
        }
        // bullet list item
        if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).or_else(|| trimmed.strip_prefix("+ ")) {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("•").color(theme::md_dim()));
                ui.add_space(4.0);
                ui.label(md_line_job(ui, &format!("{rest}{suffix}"), base, theme::md_ink()));
            });
            continue;
        }
        if t.is_empty() { ui.add_space(4.0); continue; }
        ui.label(md_line_job(ui, &format!("{t}{suffix}"), base, theme::md_ink()));
    }
}

/// Default system prompt. Tuned for a small (1B-class) instruct model running a software-
/// maintenance tool: short, concrete, imperative, with an explicit response shape and
/// anti-hallucination guards. Small models follow a tight task spec far better than a
/// vague persona, so this names the domain, the response pattern, and the hard rules.
const BASE_SYSTEM: &str = "You are Logscope, an experienced software engineer working \
alongside the user to understand, debug, and extend real systems. You handle code, logs, \
configs, build failures, runtime behavior, and architecture questions, and you produce \
files, patches, commands, and explanations.

Before answering anything non-trivial, \
think briefly first: restate the goal, weigh the likely causes, pick the most probable, \
then respond. Do not show these steps unless asked.

How to work:
- Answer the user's \
actual question first. Logs or files in context are reference material, not the task; use \
them only when relevant.
- Work only from information provided. Never invent file \
contents, log lines, paths, numbers, or output. If something is missing, say exactly what \
you need.
- Ask a clarifying question only when you are genuinely blocked; otherwise state \
your assumption in one line and proceed.
- Prefer the smallest fix that works before larger \
rewrites; preserve existing style.
- State assumptions out loud; note uncertainty plainly.
- Be concise when the answer is obvious, fuller when the problem is complex.

Style: write \
plainly and directly. No filler, no flattery, no apologizing, no restating the question back. \
Lead with the answer. Use prose for reasoning; reach for lists only when the content is \
genuinely a list.

When you produce a file, put it in a fenced block whose info line is the \
path (e.g. ```rust src/main.rs```), or precede the block with a `// file: path` line, so it \
can be saved directly. When a change spans several files, emit one block per file, each with \
its own path line. Use ```diff for patches. Use plain prose for reasoning and explanation.";

/// Appended to the system prompt when "Fix" mode is on. Turns the general assistant into the
/// step-up log/code-fix submode: the user wants the fault fixed, emitted as a saveable
/// artifact, not just discussed. Pairs with the Log toggle (which supplies the log data).
const FIX_MODE: &str = "FIX MODE is on. The user wants the fault in the log or code resolved, \
not just discussed. Respond in this shape: (1) name the specific problem in one line; (2) give \
the precise fix — the exact command, config change, or minimal code edit, emitted as a file \
(```lang path``` or a `// file: path` line) or a ```diff so it can be saved directly; (3) one \
short line on why, or what to check next. Keep it tight. Use only the paths, numbers, and \
output the user actually provided — never invent them. If the log or code is insufficient to \
locate the fault, say exactly which additional lines or files you need.";

/// The DEFAULT persona: a long-horizon engineer for a continuing session on a large codebase. Distilled
/// from the ENGINEERING MODE spec into a tight, concrete brief (small models follow a short spec far
/// better than a long one). Keeps the same file-output convention as BASE_SYSTEM so the generated-files /
/// cargo-check workspace keeps working. The router downshifts to BASE_SYSTEM (logscope) for easy log fixes.
const LONG_HORIZON: &str = include_str!("../../prompts/long_horizon.md");

/// Selectable system personas the router and the Prompting page switch between. First entry is the
/// default. Each is a full system prompt (not an action-prompt like `prompts::PROMPTS`): the long-horizon
/// engineer leads; `logscope` is the tight log/code-fix persona kept as a fast reach tool for easy fixes.
const PERSONAS: &[(&str, &str, &str)] = &[
    ("longhorizon", "Long-horizon engineer", LONG_HORIZON),
    ("logscope", "Logscope · quick log fix", BASE_SYSTEM),
    // The LIAISON runs in FRONT of the big model: a small fast model that reports what the backend is
    // doing while the large one works. Its rules live in logscope::liaison_prompt so the same wording
    // governs the HUD strip and any direct chat with it — one prompt, not two that drift.
    // Both halves. LIAISON_SYSTEM governs when there ARE observations; LIAISON_NO_OBSERVATIONS covers
    // the turn it does not bear on — which is most of them when this is selected as a chat persona,
    // and is why every reply opened with a backend status line including the answer to "hi there".
    ("liaison", "Liaison · reports backend state", logscope::liaison_prompt::LIAISON_SYSTEM),
];
/// Look up a persona's full system prompt by key (the status-bar persona popup switches with this).
fn persona_by_key(key: &str) -> Option<&'static str> { PERSONAS.iter().find(|p| p.0 == key).map(|p| p.2) }

/// Which subpage the right sidebar shows. The sidebar consolidates the per-conversation panels
/// (generated files, context/system settings, prompting modes) behind a button rail.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RightPage { Files, Graph, Context, Prompting, Engine }

/// Neutralize control-token marker strings that appear inside INJECTED CONTENT (system prompt, log
/// lines, user/assistant turns) so the tokenizer can't silently encode them as real special tokens.
/// A stray `<|im_end|>` from an open log (logscope inspects LLM logs!) would otherwise become control
/// token 151645 mid-prompt, corrupting the turn structure — the model then emits its OWN `<|im_start|>`
/// and stops almost immediately (the early-stop seen on Qwen but not Llama, since `<|im_end|>` is a
/// ChatML special, not a Llama-3 one). A zero-width space after the leading `<` breaks the exact match
/// while staying invisible in the UI and reading as ordinary text to the model. The builders' OWN
/// structural markers are written as literals and never pass through here, so they stay intact. No-op
/// for normal text (only touches content already containing a `<|` / `<｜` sigil).
fn neutralize_markers(text: &str) -> String {
    if !text.contains("<|") && !text.contains("<\u{ff5c}") { return text.to_string(); }
    text.replace("<|", "<\u{200b}|").replace("<\u{ff5c}", "<\u{200b}\u{ff5c}")
}

/// Sentinel pushed onto the generation queue by `:continue-all` — `(CONTINUE_ALL_TOK, Some(leaf))` means
/// "expand this leaf with a continuation" rather than "start a new turn with this text". Can't collide
/// with a real user message (control char + reserved word).
const CONTINUE_ALL_TOK: &str = "\u{1}__continue_all__";

/// A running depth fan from `:Nca` / `:Nfa`: keep expanding the frontier for `rounds_left` more rounds.
/// Each round runs over the CURRENT leaves; when its generations finish (queue empties) the next round
/// fires on the new frontier → depth. Hard-capped at `MAX_FAN_QUEUE` pending so a deep/wide fan can't
/// flood the device; `:stop` (or Clear) cancels it.
#[derive(Clone)]
enum FanKind { Continue, Fuzzy { width: usize, template: String } }
#[derive(Clone)]
struct Fan { kind: FanKind, rounds_left: usize, only: Option<usize> }
const MAX_FAN_QUEUE: usize = 96;

/// What a submitted composer line resolves to. Pure + module-level so the routing is unit-testable
/// without a live model or egui context.
enum Composed { Empty, Command(crate::vimmode::Command), Message(String) }

/// Classify a submitted composer line: a leading single ':' is a command (executed, not sent to the
/// model); '::' escapes a literal message that begins with a colon; anything else is a chat turn.
fn compose_input(raw: &str) -> Composed {
    let t = raw.trim();
    if t.is_empty() { return Composed::Empty; }
    if let Some(rest) = t.strip_prefix("::") { return Composed::Message(rest.to_string()); } // escaped literal ':'
    if t.starts_with(':') { return Composed::Command(crate::vimmode::parse_command(t)); }
    Composed::Message(t.to_string())
}

/// State for a `:fill` decomposition: the skeleton and its holes, so `:assemble` can splice the filled
/// bodies back. The sub-task replies come back as branches; assembly maps them positionally (the last N
/// assistant replies = the N fills, in hole order).
#[derive(Clone)]
struct FillSession { skeleton: String, holes: Vec<crate::decompose::Hole> }

/// Parse an imported chat (the markdown `:export` shape: a `## You` / `## Assistant` section per turn,
/// optional `# title`) back into an ordered list of turns. Header matching is case-insensitive and
/// accepts you/user and assistant/asst; unknown headers drop their section. Body keeps its fenced code
/// and blank lines, trimmed at the edges. Pure + linear — branches are reconstructed by the user
/// (fuzz / continue-all) after import, not encoded in the file.
fn parse_imported_chat(text: &str) -> Vec<(Role, String)> {
    fn flush(out: &mut Vec<(Role, String)>, role: Option<Role>, buf: &str) {
        if let Some(r) = role { let t = buf.trim(); if !t.is_empty() { out.push((r, t.to_string())); } }
    }
    let mut out: Vec<(Role, String)> = Vec::new();
    let mut role: Option<Role> = None;
    let mut buf = String::new();
    for line in text.lines() {
        let l = line.trim_start();
        if let Some(h) = l.strip_prefix("## ") {
            flush(&mut out, role, &buf); buf.clear();
            let h = h.trim().to_lowercase();
            role = if h.starts_with("you") || h.starts_with("user") { Some(Role::User) }
                   else if h.starts_with("assistant") || h.starts_with("asst") { Some(Role::Assistant) }
                   else { None };
        } else if role.is_none() && l.starts_with("# ") {
            // document title before the first turn — skip
        } else {
            buf.push_str(line); buf.push('\n');
        }
    }
    flush(&mut out, role, &buf);
    out
}

pub struct Chat {
    /// The accessibility scene the mode toggles register with.
    ///
    /// Parity reported `a11y=0` for this app: the registry has existed
    /// since bot-arena adopted it and nothing here fed it. Held on the
    /// tab rather than built per frame because `A11yScene` remembers
    /// focus BY ID between frames — a per-frame scene forgets, which
    /// presents as focus jumping rather than as a missing scene.
    pub a11y: uikit_egui::a11y::A11yScene,
    /// Model name for turns generated from now on; None until one is known. Deliberately stamped at
    /// creation rather than derived at render time from whatever is loaded — deriving would relabel
    /// history on every switch, which is the opposite of provenance.
    stamp_model: Option<String>,
    msgs: Vec<Msg>,
    input: String,
    context: String,   // system prompt — the "context" the model always sees
    /// Memoised `planned_files` result, keyed by (workspace root, budget).
    ///
    /// NOT an optimisation — a correctness-of-behaviour fix for a bug I
    /// introduced. `workspace_block` is reached from `est_prompt_tokens`, which
    /// the composer calls to show a live token estimate, i.e. every frame. So
    /// the planner walked the whole workspace tree and read the selected files
    /// at 60 Hz, and its diagnostic printed once per frame — which is the
    /// "workspace files: 51 considered…" line repeating forever in the log.
    ///
    /// RefCell because `workspace_block(&self)` is on the read path and making
    /// it `&mut` would push interior mutability up through several callers for
    /// no gain.
    planned_cache: std::cell::RefCell<Option<(std::path::PathBuf, usize, String)>>,
    /// The question the cached plan was built for.
    planned_q: std::cell::RefCell<Option<String>>,
    /// Why each candidate is or is not in the prompt, for the context panel.
    /// Previously this existed only as a `diag!` line giving counts without
    /// names — enough to know a selection happened, not enough to disagree
    /// with it.
    pub planned_why: std::cell::RefCell<Vec<crate::ctxplan::Explained>>,
    threading: bool,   // thread prior turns into the prompt, or single-shot
    /// Oldest turn currently in the model's window, remembered so it can STAY there.
    ///
    /// A plain tail window advances this by one turn every turn, and the prompt is
    /// `[system][oldest..newest]` — so if `oldest` moves, everything after the system block is new
    /// text and the KV prefix tree can match nothing else. Measured on a seventeen-turn log: reuse
    /// fell from 79% to 49–59% and stayed there, prefill went 5.5s -> 23s a turn and never came back.
    ///
    /// `Cell` because `chosen_msgs` is `&self` and is also called to build previews at earlier
    /// `upto` values; only the call for the REAL prompt is allowed to write, or a preview would
    /// move the floor the real prompt is trying to hold.
    win_floor: std::cell::Cell<Option<usize>>,
    attach_log: bool,  // include a bounded summary of the current log in the system prompt
    fix_mode: bool,    // step-up submode: emit a concrete fix (file/diff) for the log/code
    show_context: bool, // right sidebar open/closed
    right_page: RightPage, // active sidebar subpage
    /// What the operator has set in the Engine panel's tuning section, keyed by the
    /// environment variable the engine reads. Empty means "all defaults"; a knob the
    /// environment owns is never written here — see `engine_knobs`.
    engine_knob_values: std::collections::HashMap<String, crate::engine_knobs::Value>,
    coord_mode: crate::coordinator::Mode, // prompting mode (barebones / coordinator / aggressive)
    /// Hand-picked engineering profiles. Empty = autopilot (`engprofile::suggest` from the task).
    profiles_manual: Vec<crate::engprofile::ProfileId>,
    /// Which key cluster flies the freecam. Personalisation and accessibility: egui reports logical
    /// keys, so a non-QWERTY layout gets no movement at all from a hardcoded WASD.
    nav_layout: crate::navkeys::NavLayout,

    /// A duration the operator asked for, and when they asked.
    ///
    /// Parsed from their message by `sessiontime::parse_work_limit`, so "keep
    /// working for the next hour" becomes something the model can be told about
    /// on later turns. `None` means open-ended, which is NOT the same as
    /// expired — a zero limit would read as "stop now".
    work_limit: Option<(u64, std::time::Duration)>,
    verify_job: Option<VerifyJob>, // running sandbox build-check of the generated files
    image_job: Option<ImageJob>,   // running :image generation (sd-cli, off-thread)
    replicator: Option<ReplicatorMode>, // :replicate — parallel worker columns over the workspace
    /// The engine's outstanding compaction request, seq included so a request already declined is
    /// not re-offered every frame. None = nothing pending.
    compact_offer: Option<crate::backend::CompactRequest>,
    /// Seq of the last request the operator dismissed — dismissing means "not this one", not
    /// "never again", so a LATER request (higher seq) still asks.
    compact_declined: usize,
    /// Long-horizon: when the reply budget would be squeezed below the usable floor, stop and ask
    /// for a summary of the older turns instead of shrinking this reply (and every one after it).
    /// What to do when the reply budget would drop below usable.
    ///
    /// Was a bool. Three states, because the comments on the compaction path
    /// ask for a human in the loop and a checkbox cannot express "ask" — see
    /// `crate::relief_policy`.
    relief_policy: crate::relief_policy::ReliefPolicy,
    /// Set by Ctrl+Enter with an aimed fan: the NEXT dispatch goes to every column, then the aim
    /// resumes. One-shot rather than a mode, because "say this to everyone" is a sentence, not a
    /// state the operator has to remember to leave.
    fan_broadcast_once: bool,
    /// Narrow screens only: the fan takes the central area instead of the message log. On a width
    /// that can hold a real split this is inert (replicatorlayout::split returns Side).
    fan_fullscreen: bool,
    replica_jobs: std::collections::HashMap<crate::lanes::JobId, usize>, // lane job → replicator column
    verify_net: bool,              // allow the verify to fetch missing deps from crates.io (opt-in)
    show_files: bool,              // expand the generated-files drawer (toggled from the composer)
    hide_bottom_bar: bool,         // hide the whole composer/files/suggestions strip — reclaims the
                                    // full window height for the physical UI (spatial_view), which was
                                    // otherwise cramped/cut off behind it with no way to get it back.
    /// One-shot latch for the transcript swipe gestures (message-variant / background-branch): set
    /// once a drag has fired an action, cleared when the pointer releases. Without this, checking
    /// against `total_drag_delta()` every frame while the button stays down past the axis-lock
    /// threshold would fire the SAME swipe again on every subsequent frame of one continuous drag.
    swipe_consumed: bool,
    walk: crate::walkcam::WalkCam, // prezi-style walkthrough camera over the graph
    walk_focus: Option<usize>,     // node the walkthrough is centered on
    cam_pan_path: Vec<usize>,      // node-id path (graph shortest path) the camera is gliding along
    cam_pan_t0: Option<std::time::Instant>, // start of the current glide; None = not panning
    scroll_chat_to: Option<usize>, // pending jump-scroll of the chat log to this message index
    orbit: crate::spatial::OrbitCam, // 3D camera for the spatial message view
    pearl_input: crate::pearl_input::PearlInput, // the composer rendered as a resonance-cell pearl
    pearl_on: bool,        // show the in-scene refractive pearl (toggle in the spatial toolbar)
    /// Last press in the 3D scene: world-space hit point + the scene-clock time it landed. Feeds
    /// the shader's u_touch (world xyz + age), which existed and was DEAD — nothing ever set it
    /// (physical-ui-findings.md). None once the response has faded.
    scene_touch: Option<([f32; 3], f32)>,
    /// Node under the pointer in the 3D scene, from `scenepick` — drives the hover lift/glow (a node
    /// now visibly answers before it's clicked, not just after; see the click-site comment for why
    /// the pick itself moved to `scenepick` too).
    scene_hover: Option<usize>,
    /// Node currently PRESSED (pointer down + over it), for `scenepick::State::Pressed`'s sink+dim —
    /// the click's camera-glide only starts on release, so without this a press had zero feedback
    /// until the finger/button lifted. Same accessibility point `scenepick`'s own header makes about
    /// hover: an object should answer attention, and a press is the most immediate attention there is.
    scene_pressed: Option<usize>,
    /// PEARL PULL — the focused-turn pearl's animated "pulled toward camera" amount, 0 (resting on
    /// its strand) .. 1 (fully pulled, reading distance). Eased toward the target each frame rather
    /// than snapped, so focusing a node reads as a physical pull-and-zoom (TODO.md "PHYSICAL UI —
    /// pearl-as-message-box") rather than a teleport. Drives both the node's camera-ward offset and
    /// the scene's depth-of-field (background defocuses as the pearl comes to hand).
    focus_pull: f32,
    /// Live multi-touch points (from raw egui Touch events), keyed by touch id — feeds `touch_nav`
    /// for PINCH-ZOOM on touchscreens. Single-finger drag-orbit already works today because egui folds
    /// one touch into its ordinary pointer/`Response::dragged()` path; a pinch does not fit that model
    /// (it's two contacts, not one), which is the actual gap `touch_nav.rs`/`touchkit` exist to close —
    /// built, tested (touchkit has its own suite), and never wired to a live touch feed in this crate.
    touch_active: std::collections::HashMap<u64, egui::Pos2>,
    touch_rec: touchkit::GestureRecognizer,
    /// Mic pearl's PHYSICAL motion, driven by the real input level via `micresponse` rather than a
    /// canned sine — the leaf inertia / rune onset / bead flux distinction actually means something
    /// once the driver is real audio energy instead of a boolean "is speech happening".
    mic_motion: crate::micresponse::Motion,
    mic_prev_level: f32,
    pearl_variant: u8,     // pearl optical style: 0 balanced · 1 fine-crackle · 2 milky · 3 clear-glass
    backdrop: bool,        // render-as-full-window-backdrop: suppress spatial chrome, fill the whole rect
    glass_host: bool,      // app is in glass mode: the central canvas stays transparent (the backdrop draws the scene)
    frost_a: f32, frost_b: f32, // backdrop frost band thickness at each screen edge (UV), set per-frame by app
    spatial_view: bool,            // central area shows the 3D graph (aggressive) instead of linear chat
    spatial_auto: bool,            // camera target auto-follows the graph centroid (overview framing)
    spatial_zoomed: bool,          // user has zoomed → stop auto-fitting the distance
    pruned: std::collections::HashSet<usize>, // message indices the human pruned out of context
    region: Option<std::collections::HashSet<usize>>, // :best — the agent's highlighted best subtree
    fan: Option<Fan>,              // :Nca / :Nfa — a running depth fan over the frontier
    count_pending: Option<usize>,  // a numeric motion prefix being typed in Normal mode (e.g. 21j)
    fill_session: Option<FillSession>, // :fill — skeleton + holes, for :assemble to splice back
    active_leaf: Option<usize>,    // tip of the live branch (the message-tree head; None until first turn)
    branch_from: Option<usize>,    // if set, the next user message forks off this node instead of the tip
    gen_samples: Vec<crate::estimate::GenSample>, // completed generations → progress estimates
    last_prompt_tokens: usize,     // prompt size of the in-flight generation (for the live estimate)
    queue: std::collections::VecDeque<(String, Option<usize>)>, // (text, parent) fired while busy → run when free
    fold_pos: Vec<crate::spatial::Vec3>, // cached protein-fold layout (recomputed when the tree changes)
    flex_pos: Vec<crate::spatial::Vec3>, // cached obstacle-AVOIDED render positions (strand weaved around scenery)
    flex_key: Option<(usize, crate::scene3d::EnvParams)>, // (node count, env) the flex was computed for
    fly_auto: Option<f32>,               // interactive Tour: when Some(speed), the camera flies along the strand
    tour_s: f32,                         // accumulated strand position (variable-speed traversal — lingers on long msgs)
    tour_dwell: Vec<f32>,                // per-node dwell weight (bounded z-score of text length); recomputed on tree change
    cam_drift: Option<crate::voice_intent::MoveDir>, // "move back"/"move forward" continuous travel; cleared by stop/hold
    walk_phase: f32,                     // footstep cadence accumulator for the walking bob while drifting
    walk_bob: f32,                       // currently-applied vertical bob offset (eased in/out; delta-applied so no drift)
    walk_ramp: f32,                      // 0..1 ease-in on the drift speed so walking starts gently, not at full pelt
    fly_push: crate::spatial::Vec3,      // eased collision-avoidance offset for the fly (so evasion glides, not jolts)
    freecam: bool,                       // free-flight camera: WASD/QE move the eye, drag looks (no auto-frame/tour)
    voice: Option<crate::voice_session::VoiceSession>, // live mic session (None = off); pumped each frame in ui()
    voice_err: Option<String>,           // last mic-start error, shown inline in the composer row
    code_voice: bool,                    // coder dictation: spoken symbols → punctuation ("prose mode" turns off)
    cam_shot: u8,                  // cinematic director: current shot (0 travel,1 establish,2 orbit,3 dutch,4 push)
    cam_shot_until: f32,           // wall-clock time (s) the current shot runs until
    cam_orbit_phase: f32,          // accumulated phase for slow orbit/dutch drift
    mode: crate::vimmode::Mode,    // modal control (Normal/Insert/Command/Search) — both views now
    cmdline: String,               // the `:` command buffer
    search_q: String,              // the `/` search buffer
    search_kind: crate::vimmode::SearchKind,
    z_pending: bool,               // a `z` rotation prefix is armed
    leader_pending: bool,          // a `space` leader is armed
    plaque_scroll: crate::plaque_scroll::PlaqueScroll, // impakt-cam-style smooth scroll for the reading plaque
    plaque_last_focus: Option<usize>, // which message the plaque scroll belongs to (reset on change)
    request_smaller_model: bool,   // :why asked to spawn the smallest local model for the diagnosis; ui() applies it
    show_help: bool,               // `?` keybinding overlay is open
    note: String,                  // transient status line (e.g. export path), cleared on next action
    /// A failure that must NOT scroll away: model load errors, and anything else an operator has to
    /// act on or report.
    ///
    /// `note` is a transient status line and the next status overwrites it — which is the wrong home
    /// for the one message someone needs to keep. A load failure prints a paragraph of arithmetic
    /// (host RAM, resident, page cache, the flag that would help) and then vanishes on the next tick.
    ///
    /// Persistent, with copy and close, because the two things anyone does with an error are paste it
    /// somewhere and dismiss it.
    error_banner: Option<String>,
    gitmind_session: Option<crate::gitmind_session::Session>, // a loaded .gitmind /tmp work session, if any
    agent_files: Vec<crate::workspace_exec::FileEvent>, // live feed of files the agent/drops touched in the workspace
    agent_plan: Vec<(String, bool)>, // the agent's committed plan (2): (step text, done) — rendered as an in-scene checklist
    agent_run: Option<crate::workspace_exec::AgentRun>, // active NON-BLOCKING agent driver (channel-driven state machine)
    agent_fan: Option<crate::workspace_exec::AgentFan>, // active MULTI-branch fan (parallel agent branches, round-robin)
    agent_candidates: Vec<crate::workspace_exec::BranchSummary>, // folded branch summaries to prune to the keeper
    fan_is_docs: bool, // the active/just-finished fan is a DOC fan → suggest :merge-docs (union) over :pick (keep one)
    agent_pending_submit: Option<String>, // next agent prompt to hand the inference worker (drained in sync_worker)
    /// The tool-fence GBNF for the run in progress. Set at :agent / :fan-agent from the run's tool
    /// whitelist, cleared when the run ends; `sync_worker` sends it with every agent prompt and
    /// re-sends plain params afterwards. `LOGSCOPE_TOOL_GRAMMAR=0` leaves it off.
    agent_grammar: Option<String>,
    /// The last run's scoreboard, one line, shown in the note and kept for the status panel.
    pub harness_line: Option<String>,
    /// The bundle `:bundle` just wrote, for the Hive tab to pick up into its Integrate field.
    last_bundle: Option<std::path::PathBuf>,
    /// The plan `:survey` just produced, for the Hive tab to run.
    last_plan: Option<(String, hive::plan::Plan)>,
    /// The active model's own chat template, read from its GGUF and cached by path. Used only when
    /// `LOGSCOPE_GGUF_TEMPLATE=1`; otherwise the hand-built families below run. `Some((path, None))`
    /// records a path whose GGUF had no template, so it is not re-read every frame.
    gguf_tmpl: std::cell::RefCell<Option<(String, Option<std::sync::Arc<template_kit::Renderer>>)>>,
    agent_task: String, // the in-flight :agent task text (for the snapshot message + reaction seeding)
    lanes: Option<crate::lanes::LanePool>, // :lanes N — concurrent generation lanes over ONE resident model; fan branches decode in parallel through this instead of round-robin on the single worker
    lane_jobs: std::collections::HashMap<crate::lanes::JobId, usize>, // in-flight pool job → fan branch (routes tagged Token/Done events)
    lanes_ready: bool, // the pool's async model load completed (LaneEvt::Ready seen)
    /// Device cost of one more model instance, learned from LaneEvt::Ready. 0 = unknown.
    instance_bytes: u64,
    local_model_path: Option<String>, // the picker's selected LOCAL .gguf, captured each frame for :lanes
    rep_monitor: crate::repetition::RepeatMonitor, // cuts a runaway repetition loop in the live reply (all backends)
    stream: Option<Stream>,
    last: Option<(usize, f32)>,
    ctx: Ctx,
    backend: BackendKind,
    sent_params: Option<(u32, u32, i32, usize)>,
    sent_backend: Option<BackendKind>,
    pending_cancel: bool,
    ws_note: Option<(String, Instant)>,
    save_picker: crate::filepicker::FilePicker,
    attach_picker: crate::filepicker::FilePicker,
    save_dir: Option<std::path::PathBuf>,  // remembered "save to folder" location
    last_saved: Option<std::path::PathBuf>, // folder of the most recent save (for "Open folder")
    /// PATH of the generated file whose body is expanded, not its index.
    ///
    /// `arts` is re-parsed from the message text every frame
    /// (`parse_artifacts(last_text)`), so an index is only valid until the
    /// next reply emits an artifact before an existing one — after which the
    /// expanded pane showed a DIFFERENT file than the row that was clicked.
    /// Wrong content, silently, and only when the list happened to renumber.
    ///
    /// A path is what the row actually identifies, and `parse_artifacts`
    /// already merges fragments that share one, so two rows cannot carry the
    /// same path.
    preview: Option<String>,
    auto_continue: bool,                    // decode a fresh reply continuously up to the reply ceiling
    stepwise: bool,                         // checkpoint mode: model pauses at ⏸ STEP k/n markers, harness auto-continues
    patch_mode: bool,
    /// The model actually running, as reported by a successful load.
    last_loaded: Option<String>,
    /// What the picker was asked for, so a failure can tell a switch from a reload.
    last_picked: Option<String>,
    /// Set on failure; the app applies it to the picker on the next frame. Some(None) means clear.
    ///
    /// Deferred rather than applied here because the picker lives in the app and this is the chat
    /// tab — reaching across would be a second writer to the same state, which is the bug _290 fixed.
    revert_picker: Option<Option<String>>,
    /// Measured decode rate, for revealpace. None until a generation has been timed — which starts
    /// the reveal in token mode, the safe default on an unknown model.
    last_tok_per_s: Option<f32>,
    /// Milliseconds the current tail has been withheld, so a boundary that never arrives still flushes.
    reveal_held_ms: u64,
    /// Bytes revealed at the last frame, so the held clock resets when the boundary moves.
    reveal_shown_len: usize,
    /// Debate-panel mode: several small models instead of one thinker, able to stop and ask.
    panel_mode: bool,                       // edit mode: model emits unified-diff patches instead of full files
    step_last_paused: u32,                  // dedup: highest step number already paused on this reply
    step_continues: u32,                    // auto-continues issued for this reply (hard-capped)
    step_pending_continue: bool,            // a step pause finished → issue the continue when the stream closes
    step_arm_continue: bool,                // set by the Done handler; consumed by the frame loop
    step_was_continue: bool,                // the in-flight stream is a stepwise continuation (not a fresh reply)
    step_empty_retries: u32,                // consecutive 0-token continues nudged (bounded, anti-loop)
    step_shell: Option<std::sync::mpsc::Receiver<String>>, // in-flight cargo test at a step pause
    step_inject: Option<String>,            // shell results to splice into the next continue
    hit_cap: bool,                          // last finished reply ran the full budget (offer manual Continue)
    checkpoints: Vec<Checkpoint>,           // restorable conversation snapshots (rollback)
    show_checkpoints: bool,                 // checkpoints panel open
    /// GPU phase-breakdown subpanel open (📊 in the composer bar).
    show_metrics: bool,
    /// Tracks which step faults have already been reported, so a repeat can stop the run.
    step_verifier: crate::stepverify::Verifier,
    /// Steps the reader has opened, as (message, step). Per-step rather than per-message: opening one
    /// long step usually means wanting that one, not every step of every later reply.
    step_open: std::collections::HashSet<(usize, usize)>,
    /// Rolling window of the Thinker's recent output, for the watch strip.
    watch: logscope::thinkerwatch::ThinkerWindow,
    /// The whole current reply: the forecast must count fences opened BEFORE the window, since a code
    /// block started 300 words ago is still open and a window-local count would call it closed.
    watch_full: String,
    /// The Liaison's latest condensation, None until the first poll returns.
    watch_gist: Option<String>,
    /// Messages the reader has explicitly opened, overriding the cliffsnotes collapse. Explicit rather
    /// than sticky-by-default: a reader who opens one long reply usually wants that one, not all of them.
    expanded_msgs: std::collections::HashSet<usize>,
    /// Running backend account shown under the newest message while a reply is generating.
    hud: crate::liaisonhud::Hud,
    /// Workers known this session — the Queen/worker Delegator's roster (`docs/12-queen-and-
    /// workers.md`). Seeded, not empty, so `:agent`/`:fan-agent`/`:docs` always have the seed set to
    /// staff against; `:enlist` grows it when the Queen designs a new one. Session-lifetime only —
    /// not persisted across restarts, since a worker's memory directory (not this roster entry) is
    /// what `docs/12-queen-and-workers.md` says should survive a restart.
    roster: logscope_fan::roster::Roster,
    /// Heroes enlisted this session (finite-state workers — `logscope_fan::hero`). Session-lifetime,
    /// like the roster; handed to every agent run through `AgentConfig::extra_heroes`.
    heroes: Vec<logscope_fan::hero::Hero>,    /// The strongest thing the reply in flight may assert.
    ///
    /// Set by the routing matrix when a generation starts. Held so the
    /// reply can be recorded at its real epistemic kind rather than
    /// being assumed to be fact — which is what happened when every turn
    /// went to one model with one set of permissions.
    route_ceiling: Option<crate::liaison::Epistemic>,
    /// Generated personas. Empty until a corpus is supplied, which is
    /// why `promptroute` treats an empty roster as "no workers" rather
    /// than as an error — a fresh install has none and must still chat.
    workers: crate::workerroster::WorkerRoster,
    /// The persona prompt for the reply in flight, when this turn routed
    /// to a worker. Prepended by the prompt builder; `None` means the
    /// plain engine.
    route_persona: Option<String>,
    /// Cached context-card contents. See the draw site: this block
    /// re-scanned the transcript and re-extracted keywords every frame.
    /// Keyed on (message count, last message length) so a STREAMING
    /// reply growing in place still invalidates it.
    ctx_card_key: Option<(usize, usize)>,
    ctx_card_code: Option<String>,
    ctx_card_kws: Vec<String>,
    /// Cached context-card contents. See the draw site: this block
    /// re-scanned the transcript and re-extracted keywords on every
    /// frame.
    /// Rounds the panel has run, ever. Counts UP so
    /// `panelvoice::triage` can expire stale asks; see `fan_step`.
    panel_round: u32,
    /// Operator interjections heard while a panel is running, waiting to
    /// be triaged. Kept across frames because `triage` ranks a BATCH —
    /// a question dropped the frame it was heard cannot compete with
    /// one raised a moment later.
    pending_asks: Vec<logscope::panelvoice::Ask>,
    /// The liaison model's phrasing of the HUD, when one has produced a line. Empty = show the raw
    /// observation instead, which cannot be wrong because it is copied from the engine.
    hud_narration: String,
    /// Counter snapshot taken when the last generation STARTED, so the panel shows that generation
    /// rather than everything since process start.
    metrics_base: crate::metricspanel::Counters,
    /// Wall time of the last completed generation.
    metrics_wall_ns: u64,
    /// Tokens produced by the last generation, so per-token figures are that generation's.
    metrics_tokens: u64,
    /// When the current generation started, for measuring its wall time.
    metrics_start: Option<std::time::Instant>,
    checked_restore: bool,                  // ran the startup autosave check
    recover: Option<usize>,                 // autosave found at startup with N msgs (offer recovery)
    vault: Option<crate::vault::Vault>,     // project-history store (content-addressed tree); None if unavailable
    head: Option<crate::vault::NodeId>,     // current node in the history tree
    show_history: bool,                     // history/tree panel open
    file_hist: Option<usize>,               // generated-file index whose version history is expanded
    template: TemplateKind,                 // chat-template family of the loaded model (set on load)
    /// The worker's live template, refreshed every frame. Preferred over `template` at prompt-build
    /// time so a generate submitted in the same frame as a model load cannot use the previous model's.
    live_template: Option<TemplateKind>,
    phys: crate::physui::Panel,             // in-scene physical control dock (Overview/Tour/Freecam as 3D buttons)
    phys_anim: Vec<crate::physui::ControlAnim>, // per-dock-control eased press/hover/lit feel (indexed by DOCK_IDS)
    phys_t: Option<std::time::Instant>,     // last dock-anim tick (for dt); None until the first frame
    slider_drag: Option<(usize, f32)>,      // while dragging a physical settings slider: (dial id, last pointer x)
}
impl Default for Chat {
    fn default() -> Self {
        // Read once, here. Loading per-field would stat the config file several times per
        // construction for no benefit.
        let saved_prefs = crate::prefs::load();
        Self {
            a11y: uikit_egui::a11y::A11yScene::new(),
            stamp_model: None,
            win_floor: std::cell::Cell::new(None),
            msgs: Vec::new(), input: String::new(),
            context: LONG_HORIZON.into(),
            planned_cache: std::cell::RefCell::new(None),
            planned_q: std::cell::RefCell::new(None),
            planned_why: std::cell::RefCell::new(Vec::new()),
            threading: true, show_context: false, attach_log: true, fix_mode: false,
            right_page: RightPage::Prompting,
            engine_knob_values: Default::default(), coord_mode: crate::coordinator::Mode::Barebones,
            // PERSISTED CHOICES, restored here rather than defaulted. A Dvorak user re-selecting
            // their layout every launch is the accessibility bug returning in a slower form.
            profiles_manual: saved_prefs.profiles.clone(),
            nav_layout: saved_prefs.nav_layout,
            // Open-ended until the operator says otherwise.
            work_limit: None,
            verify_job: None,
            image_job: None,
            replicator: None,
            relief_policy: crate::relief_policy::ReliefPolicy::default(),
            compact_offer: None,
            compact_declined: 0,
            fan_broadcast_once: false,
            fan_fullscreen: false,
            replica_jobs: Default::default(),
            instance_bytes: 0,
            verify_net: false,
            show_files: false,
            hide_bottom_bar: false,
            swipe_consumed: false,
            walk: crate::walkcam::WalkCam::new(13.0), walk_focus: None, scroll_chat_to: None,
            cam_pan_path: Vec::new(), cam_pan_t0: None,
            orbit: crate::spatial::OrbitCam::default(), spatial_view: false,
            pearl_input: Default::default(),
            pearl_on: false,
            scene_touch: None,
            scene_hover: None,
            scene_pressed: None,
            focus_pull: 0.0,
            touch_active: std::collections::HashMap::new(),
            touch_rec: touchkit::GestureRecognizer::default(),
            mic_motion: crate::micresponse::Motion::default(),
            mic_prev_level: 0.0,
            pearl_variant: 0,
            backdrop: false,
            glass_host: false,
            frost_a: 0.0, frost_b: 0.0,
            spatial_auto: true, spatial_zoomed: false, pruned: std::collections::HashSet::new(),
            region: None,
            fan: None, count_pending: None,
            fill_session: None,
            active_leaf: None, branch_from: None,
            gen_samples: Vec::new(), last_prompt_tokens: 0,
            queue: std::collections::VecDeque::new(),
            fold_pos: Vec::new(),
            flex_pos: Vec::new(),
            flex_key: None,
            fly_auto: None,
            tour_s: 0.0,
            tour_dwell: Vec::new(),
            cam_drift: None,
            walk_phase: 0.0,
            walk_bob: 0.0,
            walk_ramp: 0.0,
            fly_push: crate::spatial::Vec3::new(0.0, 0.0, 0.0),
            freecam: false,
            voice: None,
            voice_err: None,
            code_voice: true,
            cam_shot: 0, cam_shot_until: 0.0, cam_orbit_phase: 0.0,
            mode: crate::vimmode::Mode::Insert, cmdline: String::new(), search_q: String::new(),
            search_kind: crate::vimmode::SearchKind::Tree, z_pending: false, leader_pending: false,
            plaque_scroll: crate::plaque_scroll::PlaqueScroll::default(),
            plaque_last_focus: None,
            request_smaller_model: false,
            show_help: false,
            note: String::new(),
            error_banner: None,
            gitmind_session: None,
            agent_files: Vec::new(),
            agent_plan: Vec::new(),
            agent_run: None,
            agent_fan: None,
            agent_candidates: Vec::new(),
            fan_is_docs: false,
            agent_pending_submit: None,
            agent_grammar: None,
            harness_line: None,
            last_bundle: None,
            last_plan: None,
            gguf_tmpl: std::cell::RefCell::new(None),
            agent_task: String::new(),
            lanes: None, lane_jobs: std::collections::HashMap::new(), lanes_ready: false,
            local_model_path: None,
            rep_monitor: crate::repetition::RepeatMonitor::default(),
            stream: None, last: None, ctx: Ctx::default(),
            backend: BackendKind::preferred(), sent_params: None, sent_backend: None, pending_cancel: false,
            ws_note: None, save_picker: Default::default(),
            attach_picker: Default::default(), save_dir: None, last_saved: None, preview: None,
            auto_continue: true, stepwise: false, patch_mode: false, panel_mode: false,
            last_tok_per_s: None, reveal_held_ms: 0, reveal_shown_len: 0,
            last_loaded: None, last_picked: None, revert_picker: None, step_last_paused: 0, step_continues: 0, step_pending_continue: false, step_arm_continue: false, step_was_continue: false, step_empty_retries: 0, step_shell: None, step_inject: None, hit_cap: false,
            checkpoints: Vec::new(), show_checkpoints: false, checked_restore: false, recover: None,
            step_verifier: crate::stepverify::Verifier::new(),
            step_open: Default::default(),
            watch: logscope::thinkerwatch::ThinkerWindow::new(),
            watch_full: String::new(), watch_gist: None,
            expanded_msgs: Default::default(),
            hud: crate::liaisonhud::Hud::new(8), hud_narration: String::new(),
            roster: logscope_fan::roster::seed_roster(),
            heroes: Vec::new(),
            route_ceiling: None,
            workers: crate::workerroster::WorkerRoster::new(),
            route_persona: None,
            ctx_card_key: None, ctx_card_code: None, ctx_card_kws: Vec::new(),
            panel_round: 0, pending_asks: Vec::new(),
            show_metrics: false, metrics_base: Default::default(), metrics_wall_ns: 0, metrics_tokens: 0, metrics_start: None,
            vault: autosave_path().and_then(|p| p.parent().map(|d| d.join("vault")))
                .and_then(|d| crate::vault::Vault::open(d).ok()),
            head: None, show_history: false, file_hist: None,
            template: TemplateKind::Llama3,
            live_template: None,
            phys: phys_dock(),
            phys_anim: vec![crate::physui::ControlAnim::default(); DOCK_IDS.len()],
            phys_t: None,
            slider_drag: None,
        }
    }
}
impl Chat {
    /// The hand-built family's markers for one system + one user turn, for `:template`'s
    /// side-by-side. Mirrors the builders above; kept separate so the comparison cannot drift with
    /// the live history.
    fn render_family_sample(&self, kind: TemplateKind, sys: &str, user: &str) -> String {
        match kind {
            TemplateKind::ChatML => format!("<|im_start|>system\n{sys}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n"),
            TemplateKind::Llama3 | TemplateKind::Llama4 => format!("<|begin_of_text|><|start_header_id|>system<|end_header_id|>\n\n{sys}<|eot_id|><|start_header_id|>user<|end_header_id|>\n\n{user}<|eot_id|><|start_header_id|>assistant<|end_header_id|>\n\n"),
            TemplateKind::Gemma => format!("<start_of_turn>user\n{sys}\n\n{user}<end_of_turn>\n<start_of_turn>model\n"),
            TemplateKind::Bailing2 => format!("<role>SYSTEM</role>{sys}\n<role>HUMAN</role>{user}\n<role>ASSISTANT</role>"),
            TemplateKind::Plain => format!("{sys}\n\nUser: {user}\n\nAssistant: "),
        }
    }

    /// Put a block of text in the transcript as a finished assistant message. For commands whose
    /// output is too long for the one-line note (`:template`), so it lands where it can be read,
    /// scrolled and exported like any other turn rather than in a status bar.
    fn push_note_block(&mut self, text: &str) {
        let mut m = Msg::assistant();
        m.variants[0] = text.to_string();
        m.think_open = false;
        m.parent = self.active_leaf;
        self.msgs.push(m);
        self.active_leaf = Some(self.msgs.len() - 1);
    }

    /// The bundle `:bundle` produced since last asked; the Hive tab drains it into Integrate.
    pub fn take_last_bundle(&mut self) -> Option<std::path::PathBuf> { self.last_bundle.take() }
    /// The plan `:survey` produced since last asked; the Hive tab drains it into its plan runner.
    pub fn take_last_plan(&mut self) -> Option<(String, hive::plan::Plan)> { self.last_plan.take() }
}

/// The GBNF for a run's tool whitelist, or None when the operator turned it off. Fans pass an
/// empty list and get the whole runtime menu, since their workers' whitelists differ per bee.
fn tool_grammar_for(tools: &[String], gate: &crate::taint::ToolGate) -> Option<String> {
    if std::env::var("LOGSCOPE_TOOL_GRAMMAR").as_deref() == Ok("0") { return None; }
    let names: Vec<&str> = if tools.is_empty() { crate::workspace_exec::RUNTIME_TOOLS.to_vec() } else { tools.iter().map(|s| s.as_str()).collect() };
    // The gate's run token goes INTO the grammar: the prompt teaches ```tool nonce="…" and the gate
    // refuses a fence without it, so a grammar that forbade the attribute vetoed every authored
    // call (template_kit::constrain's module note has the mechanics).
    let nonce = match gate { crate::taint::ToolGate::Required(n) => Some(n.as_str().to_string()), crate::taint::ToolGate::Open => None };
    Some(crate::llm_remote::tool_fence_grammar_nonced(&names, nonce.as_deref()))
}

impl Chat {
    /// Prefill the composer (used when another tab routes a prompt to Chat).
    pub fn set_input(&mut self, s: String) { self.input = s; }

    /// Cues for the living-tree ambience: (graph size, generations in flight + queued). The canopy's
    /// fullness tracks the graph size; the leaves rustle/shimmer with the generation activity.
    pub fn canopy_cues(&self) -> (usize, usize) {
        let active = self.stream.is_some() as usize + self.queue.len() + self.fan.as_ref().map_or(0, |_| 1);
        (self.msgs.len(), active)
    }

    /// A compact live summary of in-flight generation work for the status bar: whatever combination of a
    /// streaming reply, a queue of pending (branch/fan) turns, and a running depth fan is active right now.
    /// `None` when idle. Pure so the wording is unit-testable; the status bar renders it with a ⏹ stop
    /// control. Unlike `self.note` (which any later message overwrites), this reflects live state every frame,
    /// so a multi-round `:Nca`/`:Nfa` fan stays visible until it drains.
    fn activity_status(&self) -> Option<String> {
        // headless render hook (like LOGSCOPE_MIC_PEARL_DEBUG): force the HUD text so the status-bar
        // harness can screenshot the fan/queue indicator + stop control without a live model.
        if let Ok(s) = std::env::var("LOGSCOPE_FAN_DEBUG") { if !s.is_empty() { return Some(s); } }
        let mut parts: Vec<String> = Vec::new();
        if self.stream.is_some() { parts.push("generating".into()); }
        // live progress summary (refreshed ~5s from the streaming tail): what the turn is doing RIGHT now
        if let Some(s) = &self.stream {
            if let Some((sum, _)) = &s.progress {
                if !sum.is_empty() { parts.push(format!("… {sum}")); }
            }
        }
        let q = self.queue.len();
        if q > 0 { parts.push(format!("{q} queued")); }
        if let Some(f) = &self.fan {
            if f.rounds_left > 0 {
                parts.push(format!("{} {} left", f.rounds_left, if f.rounds_left == 1 { "round" } else { "rounds" }));
            }
        }
        if parts.is_empty() { None } else { Some(parts.join(" · ")) }
    }

    /// The active system persona's short label, for the status badge. Matches the current system prompt
    /// against the `PERSONAS` table (long-horizon engineer by default, or the logscope quick-fix persona);
    /// a user-edited context that matches neither reads as "custom". Pure + testable.
    fn active_persona_label(&self) -> &'static str {
        let sys = self.sys();
        PERSONAS.iter().find(|p| p.2.trim() == sys).map(|p| p.1).unwrap_or("custom · system")
    }

    /// Record the current conversation + its derived files as a node in the history tree. Files are
    /// linked to the assistant turn (and thus the reasoning) that produced them. Cheap: messages are
    /// content-addressed and shared with the parent, so only the new turn's blobs are written.
    fn vault_commit(&mut self) {
        let Some(v) = self.vault.as_mut() else { return };
        if self.msgs.is_empty() { return; }
        let pairs: Vec<(String, String)> = self.msgs.iter()
            .map(|m| ((if m.role == Role::Assistant { "assistant" } else { "user" }).to_string(), m.text().to_string()))
            .collect();
        let produced_turn = self.msgs.iter().rposition(|m| m.role == Role::Assistant)
            .unwrap_or(pairs.len().saturating_sub(1));
        let files = crate::workspace::parse_artifacts(self.msgs.get(produced_turn).map(|m| m.text()).unwrap_or(""));
        let label = self.msgs.iter().rev().find(|m| m.role == Role::User)
            .map(|m| m.text().chars().take(48).collect::<String>().replace('\n', " "))
            .unwrap_or_else(|| "turn".into());
        if let Ok(id) = v.commit(self.head, &label, &pairs, &files, produced_turn, &[], "") {
            self.head = Some(id);
        }
    }

    /// Return to a node in the history tree: snapshot the current line, load that node's transcript, and
    /// point head at it so the next turn forks a new branch from there.
    fn return_to_node(&mut self, id: crate::vault::NodeId) {
        let Some(v) = self.vault.as_ref() else { return };
        let Ok(t) = v.transcript(id) else { return };
        if !self.msgs.is_empty() { self.checkpoint("before branch"); }
        self.msgs = t.into_iter().map(|(role, text)| {
            let mut m = if role == "assistant" { Msg::assistant() } else { Msg::user(String::new()) };
            *m.cur_mut() = text;
            m
        }).collect();
        self.resync_tree();
        self.head = Some(id);
        self.hit_cap = false; self.last = None;
        self.preview = None; self.file_hist = None;
        self.autosave();
    }

    pub fn apply_evt(&mut self, e: Evt) {
        // FAN INTERCEPT: a multi-branch agent fan owns the worker while active — route to the scheduler,
        // which rotates branches round-robin on each Done.
        if self.agent_fan.is_some() {
            use crate::workspace_exec::FanStep;
            match e {
                Evt::Token(t) => { if let Some(f) = self.agent_fan.as_mut() { f.on_token(&t); } return; }
                Evt::Done { .. } => {
                    let step = self.agent_fan.as_mut().map(|f| f.on_done());
                    match step {
                        Some(FanStep::Submit { branch, prompt }) => {
                            if let Some(f) = self.agent_fan.as_ref() {
                                self.note = if f.reviewing() { format!("fan-agent: reviewing branch {} · {} branches", branch + 1, f.len()) }
                                            else { format!("fan-agent: branch {} generating · {} branches", branch + 1, f.len()) };
                            }
                            self.agent_pending_submit = Some(prompt);
                        }
                        Some(FanStep::AllDone) => self.finish_agent_fan(),
                        None => {}
                    }
                    return;
                }
                Evt::Error(err) => {
                    let step = self.agent_fan.as_mut().map(|f| f.on_error(err));
                    match step {
                        Some(FanStep::Submit { prompt, .. }) => self.agent_pending_submit = Some(prompt),
                        Some(FanStep::AllDone) => self.finish_agent_fan(),
                        None => {}
                    }
                    return;
                }
                Evt::Loaded(Ok(info)) => {
                    // BOTH, always. live_template is the OVERRIDE build_prompt reads
                    // (`live_template.unwrap_or(template)`), and these handlers `return` before the
                    // update loop's refresh — so a value left from a PREVIOUS model survived a model
                    // switch and beat the new model's detection. That is how a Bailing2 model
                    // received Llama3 markers with zero specials: the tab had loaded a Llama model
                    // earlier in the session. Tying the override to the loaded model fixes it.
                    self.template = info.template;
                self.live_template = Some(info.template);   // see the Evt::Loaded comment above
                    self.live_template = Some(info.template);
                    return;
                }
                Evt::Loaded(Err(_)) => { return; }
            }
        }
        // AGENT INTERCEPT: while a non-blocking agent run is active, its generation owns the Evt stream —
        // route tokens/turns to the driver instead of the chat message log. (The two can't run at once: one
        // model, one worker.)
        if self.agent_run.is_some() {
            match e {
                Evt::Token(t) => { if let Some(r) = self.agent_run.as_mut() { r.on_token(&t); } return; }
                Evt::Done { .. } => {
                    let stepv = self.agent_run.as_mut().map(|r| r.on_done());
                    match stepv {
                        Some(crate::workspace_exec::AgentStep::Submit(prompt)) => {
                            // reflect live progress, then queue the next turn's prompt for sync_worker
                            if let Some(r) = self.agent_run.as_ref() {
                                self.agent_files = r.events().to_vec();
                                self.agent_plan = r.plan().steps.iter().map(|s| (s.text.clone(), s.done)).collect();
                                let ph = r.phase().map(|p| format!(" · phase: {p}")).unwrap_or_default();
                                // Compaction is invisible otherwise, and "the model forgot what it
                                // read" is exactly the symptom it produces — so it is said out loud.
                                let cmp = match r.compacted_turns() { 0 => String::new(), n => format!(" · {n} turn(s) compacted") };
                                self.note = format!("agent: turn {} · {} file(s){ph}{cmp}…", r.turns(), r.events().len());
                            }
                            self.agent_pending_submit = Some(prompt);
                        }
                        Some(crate::workspace_exec::AgentStep::Finished(stop)) => self.finish_agent_run(stop),
                        None => {}
                    }
                    return;
                }
                Evt::Error(err) => {
                    let stepv = self.agent_run.as_mut().map(|r| r.on_error(err));
                    if let Some(crate::workspace_exec::AgentStep::Finished(stop)) = stepv { self.finish_agent_run(stop); }
                    return;
                }
                Evt::Loaded(Ok(info)) => {
                    // BOTH, always. live_template is the OVERRIDE build_prompt reads
                    // (`live_template.unwrap_or(template)`), and these handlers `return` before the
                    // update loop's refresh — so a value left from a PREVIOUS model survived a model
                    // switch and beat the new model's detection. That is how a Bailing2 model
                    // received Llama3 markers with zero specials: the tab had loaded a Llama model
                    // earlier in the session. Tying the override to the loaded model fixes it.
                    self.template = info.template;
                    self.live_template = Some(info.template);
                    return;
                }
                Evt::Loaded(Err(_)) => { return; }
            }
        }
        match e {
            Evt::Token(t) => {
                self.watch.push(&t);
                self.watch_full.push_str(&t);
                let mut stop = false;
                let plain = self.backend != BackendKind::Wgpu; // template path stops on <|eot_id|>
                let mut check_text: Option<String> = None;
                if let Some(s) = &mut self.stream {
                    s.tokens += 1;
                    let ntok = s.tokens;
                    if let Some(m) = self.msgs.get_mut(s.idx) {
                        m.cur_mut().push_str(&t);
                        if plain {
                            let cut = ["\nUser:", "\nuser:", "\nUSER:"].iter().filter_map(|mk| m.text().find(mk)).min();
                            if let Some(cut) = cut { m.cur_mut().truncate(cut); stop = true; }
                        }
                        // sample the reply periodically for a repetition loop (cheap; bounded window)
                        if !stop && ntok % 24 == 0 { check_text = Some(m.text().to_string()); }
                        // LIVE PROGRESS: every ~5s refresh a one-line summary of the latest activity from
                        // the streaming tail, shown as a temporary status while the turn runs (so a long or
                        // quiet coordinator turn still shows motion instead of an empty message body).
                        let due = s.progress.as_ref().map(|(_, at)| at.elapsed().as_secs_f32() >= 5.0).unwrap_or(true);
                        if due {
                            if let Some(sum) = summarize_progress(m.text()) {
                                s.progress = Some((sum, Instant::now()));
                            }
                        }
                    }
                }
                // Stepwise checkpoints: a fresh ⏸ STEP k/n marker in the stream pauses generation here.
                // The Done handler then auto-continues (warm-KV resume — a pause costs ~nothing), giving a
                // natural injection point per step and a checklist for the UI. Scan the live text directly
                // (cheap: markers are line-anchored and we only look when streaming).
                if !stop && self.stepwise {
                    if let Some(s) = &self.stream {
                        if let Some(m) = self.msgs.get(s.idx) {
                            if let Some(mark) = crate::stepwise::should_pause(m.text(), self.step_last_paused) {
                                self.step_last_paused = mark.k;
                                // Continue policy: k==n no longer stops on its own (models under-plan);
                                // an explicit TASK_COMPLETE ends the run, with a bounded grace past the
                                // nominal plan so an under-planned reply still finishes. cap is the loop
                                // backstop; grace bounds overshoot beyond n without an explicit finish.
                                let cont = crate::stepwise::should_continue_ex(&mark, m.text(), self.step_continues, 24, crate::stepwise::DEFAULT_GRACE);
                                // VERIFY BEFORE ADVANCING. A 30-turn run on a 1B model produced the
                                // same broken snippet every turn because nothing ever checked whether
                                // the code parsed — the prompts varied only in tone. prechecks has been
                                // able to catch exactly that since _240 and had no caller.
                                //
                                // Runs on every step, not only ones whose summary mentions tests: a
                                // step that does not say "test" is precisely the one that advances with
                                // a dangling `else if` and takes the next step down with it.
                                let mut cont = cont;
                                if cont {
                                    let blocking: Vec<String> = crate::stepwise::extract_files(m.text())
                                        .iter()
                                        .flat_map(|(name, body)| {
                                            crate::prechecks::scan_rust(body).into_iter()
                                                .filter(|f| f.confidence == crate::prechecks::Confidence::Certain)
                                                .map(move |f| format!("{name}:{}: {} — {}", f.line, f.what, f.fix))
                                        })
                                        .collect();
                                    match self.step_verifier.check(&blocking) {
                                        crate::stepverify::Verdict::Advance => {}
                                        crate::stepverify::Verdict::Revise { prompt, findings } => {
                                            // Reuse the channel the cargo hook already uses to reach
                                            // the model. A second mechanism would have been a field
                                            // nobody reads — which is how prechecks ended up with zero
                                            // callers in the first place.
                                            self.step_inject = Some(prompt);
                                            self.note = format!("step {}/{} — {} error(s), revising",
                                                mark.k, mark.n, findings.len());
                                        }
                                        crate::stepverify::Verdict::Stuck { repeated } => {
                                            // The same fault survived being reported. Thirty more turns
                                            // will not fix it; stopping is the useful answer.
                                            cont = false;
                                            self.note = format!("step {}/{} — stopped: {} fault(s) \
survived being reported, so more turns will not fix them", mark.k, mark.n, repeated.len());
                                        }
                                    }
                                }
                                self.step_pending_continue = cont;
                                self.note = format!("step {}/{} — {}", mark.k, mark.n, mark.summary);
                                // tests→cargo hook: a test/build step pauses, we materialize the reply's
                                // `// file:` blocks into a scratch workspace and run `cargo test` OFF-thread;
                                // the tail of its output is spliced into the continue so the model sees real
                                // compiler/test results at exactly the boundary it created.
                                if cont && crate::stepwise::step_wants_tests(&mark.summary) && self.step_shell.is_none() {
                                    let files = crate::stepwise::extract_files(m.text());
                                    if !files.is_empty() {
                                        let (tx, rx) = std::sync::mpsc::channel();
                                        self.step_shell = Some(rx);
                                        std::thread::spawn(move || {
                                            let dir = std::env::temp_dir().join(format!("logscope-step-{}", std::process::id()));
                                            let _ = std::fs::remove_dir_all(&dir);
                                            for (path, body) in &files {
                                                let p = dir.join(path);
                                                if let Some(parent) = p.parent() { let _ = std::fs::create_dir_all(parent); }
                                                let _ = std::fs::write(&p, body);
                                            }
                                            let out = std::process::Command::new("cargo")
                                                .args(["test", "--offline"]).current_dir(&dir)
                                                .output();
                                            let msg = match out {
                                                Ok(o) => {
                                                    let all = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
                                                    let tail: String = all.lines().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                                                    format!("(harness ran `cargo test` on the files so far — {}; output tail:)\n{}",
                                                        if o.status.success() { "PASSED" } else { "FAILED" }, tail)
                                                }
                                                Err(e) => format!("(harness could not run cargo test: {e})"),
                                            };
                                            let _ = tx.send(msg);
                                        });
                                    }
                                }
                                // Tripwire self-review: if we're continuing and NOT already running a
                                // cargo-test check for this pause (that's the stronger ground-truth signal,
                                // prefer it), scan the code so far for mechanical breaks. On a hit, queue a
                                // ⟲ CHECK self-review nudge so the model looks back and fixes it in place on
                                // resume — a review point offered, not a correction imposed.
                                // Only when nothing more definite is already queued. A prechecks
                                // finding is a fact about the code; the tripwire note is a nudge to
                                // self-review. Letting the nudge overwrite the fact would send the
                                // model to "look over your work" when the answer is "line 4 does not
                                // parse" — which is the tone-instead-of-correctness failure again.
                                if cont && self.step_shell.is_none() && self.step_inject.is_none() {
                                    let sig = crate::tripwire::scan(m.text());
                                    if let Some(note) = crate::tripwire::review_note(&sig) {
                                        self.step_inject = Some(note);
                                        self.note = "review point — model will self-check the code".into();
                                    }
                                }
                                stop = true;
                            }
                        }
                    }
                }
                // Repetition recovery (all backends): if the reply has locked into a loop, cut it now so the
                // model's speed stops being a runaway train instead of burning the whole token budget.
                if let Some(txt) = check_text {
                    if matches!(self.rep_monitor.observe(&txt).1, crate::repetition::Recovery::Cut) {
                        stop = true;
                        self.note = "cut a repetition loop (model was looping)".into();
                    }
                }
                if stop {
                    if let Some(s) = self.stream.take() { self.last = Some((s.tokens, s.started.elapsed().as_secs_f32())); }
                    self.pending_cancel = true;
                }
            }
            Evt::Done { tokens, secs } => {
                // Close the metrics window: prefer the worker's own timing over our wall clock, since it
                // excludes UI frame time, and fall back to the clock if it reports nothing.
                self.metrics_tokens = tokens as u64;
                self.metrics_wall_ns = if secs > 0.0 { (secs as f64 * 1e9) as u64 }
                    else { self.metrics_start.map(|t| t.elapsed().as_nanos() as u64).unwrap_or(0) };
                self.metrics_start = None;
                let idx = self.stream.as_ref().map(|s| s.idx);
                if let Some(i) = idx {
                    if let Some(m) = self.msgs.get_mut(i) {
                        if m.text().is_empty() { *m.cur_mut() = "[stopped]".into(); }
                    }
                }
                self.last = Some((tokens, secs));
                // APPLY THE PATCHES. patch_mode injected PATCH_MODE_PROMPT and then did nothing with the
                // reply — the model was asked for unified diffs that were never parsed, so "patch mode" was a
                // prompt change with no mechanism behind it. parse_patches and apply_file_patch_strict both
                // existed already; only this call was missing.
                if self.patch_mode {
                    if let Some(txt) = idx.and_then(|k| self.msgs.get(k)).map(|m| m.text().to_string()) {
                        self.apply_patch_reply(&txt);
                    }
                }
                // A finished stream reveals everything; the held clock must not carry into the next turn.
                self.reveal_held_ms = 0;
                self.reveal_shown_len = 0;
                self.stream = None;
                self.rep_monitor.reset();
                // Stepwise: the pause we requested has landed — arm the auto-continue. It fires in the
                // frame loop (continue_generation), resuming the SAME reply on warm KV. Fan-out results
                // for this step boundary can be spliced into the continue prompt here later.
                if self.step_pending_continue {
                    self.step_pending_continue = false;
                    self.step_continues += 1;
                    self.step_arm_continue = true;
                }
                // Trailing-solicitation catch: small models often END a step by ASKING to continue
                // ("Let me know when you're ready to proceed") instead of writing a marker or
                // TASK_COMPLETE. With stepwise/auto-continue on that's a soft "keep going", not a real
                // stop — so if nothing else armed a continue, the reply isn't complete, and we're under
                // the loop cap, treat the ask as a continue. (The continue prompt strips the trailing
                // question so the model doesn't re-read its own "let me know" and stall again.)
                if (self.stepwise || self.auto_continue) && !self.step_arm_continue {
                    if let Some(m) = idx.and_then(|i| self.msgs.get(i)) {
                        let t = m.text();
                        if self.step_continues < 24
                            && !crate::stepwise::signals_complete(t)
                            && crate::stepwise::trailing_solicitation(t)
                        {
                            self.step_continues += 1;
                            self.step_arm_continue = true;
                            self.note = "auto-continue: model asked to proceed — continuing".into();
                        }
                    }
                }
                // Empty-continuation catch: the model emitted its ⏸ STEP marker as the LAST thing (an
                // announcement, no step work) and then the warm-resume immediately hit EOS → 0 new tokens,
                // and the harness silently stopped mid-plan. Seen on 4B: planned 6 steps, marked STEP 1/6,
                // produced nothing, stalled. When a CONTINUATION comes back near-empty, the run isn't
                // complete, and we're mid-plan, splice an explicit "do the next step's actual work" nudge
                // and retry — bounded so a truly-finished model can't loop.
                if self.step_was_continue && tokens <= 1 && !self.step_arm_continue {
                    if let Some(m) = idx.and_then(|i| self.msgs.get(i)) {
                        let t = m.text();
                        let mid_plan = crate::stepwise::last_step_mark(t).map(|mk| mk.k < mk.n).unwrap_or(false);
                        if self.step_empty_retries < 2
                            && !crate::stepwise::signals_complete(t)
                            && mid_plan
                        {
                            self.step_empty_retries += 1;
                            self.step_continues += 1;
                            self.step_arm_continue = true;
                            self.step_inject = Some(crate::stepwise::CONTINUE_NUDGE.to_string());
                            self.note = "auto-continue: step marker had no work — nudging the next step".into();
                        }
                    }
                }
                if tokens > 1 { self.step_empty_retries = 0; } // real progress resets the anti-loop guard
                self.step_was_continue = false;
                if self.last_prompt_tokens > 0 { // bank this run for future progress estimates
                    self.gen_samples.push(crate::estimate::GenSample { prompt_tokens: self.last_prompt_tokens, output_tokens: tokens, secs });
                    let n = self.gen_samples.len();
                    if n > 64 { self.gen_samples.drain(..n - 64); }
                    self.last_prompt_tokens = 0;
                }
                // With auto-continue on, the reply was already decoded continuously up to the ceiling
                // in one session, so there's nothing to re-arm. If it ran the full budget (vs stopping
                // on eos) it may be cut off mid-thought — remember that so a manual "⏭ Continue"
                // (warm-resume) affordance can show.
                let capped = tokens >= self.ctx.max_tokens as usize
                    && idx.and_then(|i| self.msgs.get(i)).map(|m| m.role == Role::Assistant && !m.text().is_empty()).unwrap_or(false);
                self.hit_cap = capped;
                self.autosave(); // persist the completed turn for crash recovery
                self.vault_commit(); // record the turn as a node in the history tree (provenance-linked)
                if let Some(i) = idx { self.auto_verify_after_reply(i); } // compile-check generated Rust now, not on a click
            }
            Evt::Error(e) => {
                if let Some(s) = &self.stream {
                    if let Some(m) = self.msgs.get_mut(s.idx) {
                        if m.text().is_empty() { *m.cur_mut() = format!("[error] {e}"); } else { let x = format!("\n[error] {e}"); m.cur_mut().push_str(&x); }
                    }
                }
                self.stream = None;
                self.rep_monitor.reset();
            }
            Evt::Loaded(Ok(info)) => {
                self.template = info.template;
                // Remember what is ACTUALLY running, so a later failure has something to revert to.
                // Without this the picker is the only record of the model, and it is the record that
                // is wrong after a failure.
                self.last_loaded = Some(info.path.clone());
                // Surface the model's declared architecture gaps through the SAME grounded channel
                // everything else on this strip uses, rather than leaving them only in the terminal
                // (`wgpu-engine: ⚠ ...`). `docs/12-queen-and-workers.md`'s liaison audit named this
                // exact gap: the engine computed it, nothing showed it to the operator.
                if let Some(status) = &info.model_status {
                    if !status.is_fully_supported() {
                        for gap in &status.gaps {
                            self.hud.observe(format!("{}: {gap}", status.name), "loader");
                        }
                    }
                }
            }
            Evt::Loaded(Err(e)) => {
                // The banner, not the note. This message carries the arithmetic — host RAM,
                // resident, page cache, and the flag that would help — and it went to the
                // terminal while the UI showed a line the next status overwrote.
                self.error_banner = Some(e.to_string());
                // The picker was set optimistically when the load STARTED, so a failure leaves it
                // naming a model that is not running — and the menu then asserts the wrong model is
                // loaded. A name in the active slot is the one piece of state nobody re-checks.
                self.revert_picker =
                    Some(logscope::activemodel::on_load_failed(
                        self.last_picked.as_deref(), self.last_loaded.as_deref()));
            }
        }
    }

    /// Hand the pending revert to the app, once.
    ///
    /// The chat tab observes the failure and the app owns the picker; passing the decision across
    /// rather than writing it directly keeps a single writer, which is what _290 had to restore after
    /// two of them disagreed.
    pub fn take_revert_picker(&mut self) -> Option<Option<String>> { self.revert_picker.take() }

    fn params(&self) -> GenParams {
        // Auto-continue → decode a fresh reply continuously up to N chunks in one session; the slider
        // value stays the per-chunk / manual-continue size.
        // A pending compaction under `Ask` collapses the ceiling to a single
        // chunk, which ends the auto-continue chain at the next boundary and
        // lets the offer actually be seen.
        //
        // The offer already existed; auto-continue made it unreachable.
        // `compact_poll` only raises it when the stream has ended, and a
        // continuous run never has that gap — 113 requests across 120 turns,
        // none of them answerable. See `relief_policy::should_pause_auto_continue`.
        let paused = crate::relief_policy::should_pause_auto_continue(
            self.relief_policy, self.compact_offer.is_some());
        let ceiling = if self.auto_continue && !paused {
            self.ctx.max_tokens * (1 + MAX_AUTO_CONTINUE)
        } else {
            self.ctx.max_tokens
        };
        GenParams { max_tokens: self.ctx.max_tokens as usize, reply_ceiling: ceiling as usize, n_ctx: self.ctx.n_ctx, n_batch: self.ctx.n_ctx,
            temperature: self.ctx.temperature, repeat_penalty: self.ctx.repeat_penalty, min_p: self.ctx.min_p,
            // Long-horizon relief: only meaningful when auto-continue is on (that is the mode
            // whose replies are long enough for the squeeze to cost an answer), so the toggle is
            // gated on it rather than being a second setting that silently does nothing.
            // `Ask` yields None and falls back to the OLD behaviour rather
            // than to summarize: an unanswered question must not silently pick
            // the more destructive option. The prompt is raised separately.
            summarize_instead_of_squeeze: self.auto_continue
                && self.relief_policy.engine_flag().unwrap_or(false),
            n_threads: 4, n_gpu_layers: -1, grammar: None }
    }

    /// The loaded model's template, preferring the worker's live value over the per-frame copy.
    fn live_template(&self) -> TemplateKind { self.live_template.unwrap_or(self.template) }

    /// Context for command-argument completion. The ref-taking commands (:goto, :why, :mergectx,
    /// :segment, …) address messages by short id, and until now the palette offered only the static
    /// "-1/-2/-3", so picking a message meant reading its id out of the transcript by eye. Handing
    /// the real conversation over lets the palette complete an id, annotate a positional ref with
    /// the message it lands on, and find a message by what it SAYS.
    fn arg_ctx(&self) -> crate::cmdpalette::ArgCtx {
        let msgs = self.msgs.iter().enumerate().map(|(i, m)| crate::cmdpalette::MsgRef {
            idx: i,
            short_id: logscope_fan::vimmode::short_id(i),
            role: if m.role == Role::User { "you".into() } else { "assistant".into() },
            // One line is enough to recognise a message by; the palette truncates further.
            preview: m.text().lines().find(|l| !l.trim().is_empty()).unwrap_or("").to_string(),
        }).collect();
        crate::cmdpalette::ArgCtx {
            models: Vec::new(),
            refs: vec!["-1".into(), "-2".into(), "-3".into()],
            msgs,
        }
    }

    fn build_prompt(&self, doc: &crate::analysis::Doc) -> String { self.build_prompt_upto(self.msgs.len(), doc) }

    /// The same history as `build_prompt`, as structured turns for a backend that templates them
    /// itself (Remote → llama-server). System first, then the chosen messages; no assistant primer,
    /// the server adds it. Tool-role turns are not distinguished here (the chat is plain Q&A); the
    /// agent loop, which does have tool results, builds its own turns.
    fn build_turns(&self, doc: &crate::analysis::Doc) -> Vec<crate::backend::ChatTurn> {
        use crate::backend::ChatTurn;
        let sys = neutralize_markers(&self.full_system(doc));
        let mut turns = Vec::new();
        if !sys.trim().is_empty() { turns.push(ChatTurn::system(sys.clone())); }
        for m in self.chosen_msgs(8, &sys, self.msgs.len()) {
            let text = neutralize_markers(m.text());
            turns.push(if m.role == Role::User { ChatTurn::user(text) } else { ChatTurn::assistant(text) });
        }
        turns
    }

    /// Every branch tip: a message id that is no other message's (effective) parent. The effective parent
    /// is the recorded `parent`, or the previous turn for pre-branch linear history — the same rule the
    /// convo-graph uses. These are exactly the nodes `:continue-all` fans.
    fn leaf_ids(&self) -> Vec<usize> {
        let n = self.msgs.len();
        if n == 0 { return Vec::new(); }
        let mut is_parent = vec![false; n];
        for i in 0..n {
            let p = self.msgs[i].parent.or(if i > 0 { Some(i - 1) } else { None });
            if let Some(pi) = p { if pi < n { is_parent[pi] = true; } }
        }
        (0..n).filter(|&i| !is_parent[i]).collect()
    }

    /// The live leaf that descends from `anc` (following recorded/linear parentage) — the current tip of
    /// the line rooted at `anc`. For a single continued line there's exactly one; `:continue` uses this to
    /// keep deepening the SAME line across rounds. Falls back to `anc` if it is itself a leaf.
    fn leaf_under(&self, anc: usize) -> Option<usize> {
        if anc >= self.msgs.len() { return None; }
        self.leaf_ids().into_iter().find(|&l| {
            let mut cur = Some(l);
            while let Some(i) = cur {
                if i == anc { return true; }
                cur = self.msgs.get(i).and_then(|m| m.parent).or(if i > 0 { Some(i - 1) } else { None });
                if cur == Some(i) { break; }
            }
            false
        })
    }

    /// Each message's effective parent (recorded, else previous turn) — the tree the convo-graph and the
    /// best-region selector both read.
    fn effective_parents(&self) -> Vec<Option<usize>> {
        (0..self.msgs.len()).map(|i| self.msgs[i].parent.or(if i > 0 { Some(i - 1) } else { None })).collect()
    }

    /// Per-node verdict for `:best`. STRUCTURAL PROXY for now: an assistant turn that produced a fenced
    /// code block counts as a candidate (Pass); everything else is unjudged (None). Swap this for the
    /// sandbox `cargo check` oracle (per node) and the same selection becomes "largest mostly-COMPILING
    /// region" instead of "largest code-producing region" — no change to bestregion.
    fn node_verdicts(&self) -> Vec<crate::bestregion::Verdict> {
        use crate::bestregion::Verdict;
        self.msgs.iter().map(|m| {
            if m.role == Role::Assistant && m.text().contains("```") { Verdict::Pass } else { Verdict::None }
        }).collect()
    }

    /// Run ONE round of the active fan over the current leaves, then decrement. Re-invoked from the queue
    /// drain when a round's generations finish, so successive rounds deepen the frontier (`:Nca`/`:Nfa`
    /// depth). Bounded by MAX_FAN_QUEUE so a deep/wide fan paces itself instead of flooding the device.
    fn fan_step(&mut self) {
        let Some(mut fan) = self.fan.take() else { return; };
        if fan.rounds_left == 0 { return; }
        // MONOTONIC round number, separate from `rounds_left`.
        //
        // `panelvoice::triage` expires an ask after ASK_TTL_ROUNDS by
        // comparing round numbers, so it needs one that COUNTS UP.
        // `rounds_left` counts down and resets whenever a new fan
        // starts — using it would make an interjection from a long fan
        // look newer than one from a short fan that started later.
        self.panel_round = self.panel_round.saturating_add(1);
        // `only` anchors the fan to ONE line (`:continue`): each round descends to the current live tip
        // under the anchor, so it deepens that line linearly even when other branches exist. `None` fans
        // the whole frontier (`:ca`/`:fa`).
        let leaves: Vec<usize> = match fan.only {
            Some(anc) => self.leaf_under(anc).into_iter().collect(),
            None => self.leaf_ids(),
        };
        let mut added = 0usize;
        'outer: for &l in &leaves {
            match &fan.kind {
                FanKind::Continue => {
                    if self.queue.len() >= MAX_FAN_QUEUE { break 'outer; }
                    self.queue.push_back((CONTINUE_ALL_TOK.to_string(), Some(l)));
                    added += 1;
                }
                FanKind::Fuzzy { width, template } => {
                    let leaf_text: String = self.msgs.get(l).map(|m| m.text().chars().take(200).collect()).unwrap_or_default();
                    let task = crate::vimmode::fill_template(template, &leaf_text, l);
                    if task.trim().is_empty() { continue; }
                    for v in crate::consensus::fuzz(&task, *width, now_unix().wrapping_add(l as u64)) {
                        if self.queue.len() >= MAX_FAN_QUEUE { break 'outer; }
                        self.queue.push_back((v, Some(l)));
                        added += 1;
                    }
                }
            }
        }
        fan.rounds_left -= 1;
        let rounds = fan.rounds_left;
        if rounds > 0 && added > 0 { self.fan = Some(fan); } // more rounds → keep fanning as the queue drains
        self.note = if added == 0 { "fan finished — no more leaves to expand".into() }
                    else { format!("fan: +{added} queued · {rounds} rounds left · {} pending (:stop to cancel)", self.queue.len()) };
    }

    /// Replace the conversation with an imported linear chat (from `:open`). Checkpoints first so it's
    /// undoable, then re-seats the live tip.
    fn load_imported(&mut self, turns: Vec<(Role, String)>) {
        if !self.msgs.is_empty() { self.checkpoint("before: import"); }
        self.msgs.clear(); self.pruned.clear(); self.queue.clear(); self.fold_pos.clear(); self.last = None;
        let mut parent = None;
        for (role, text) in turns {
            let mut m = if role == Role::User { Msg::user(text) } else { let mut a = Msg::assistant(); a.variants[0] = text; a };
            m.parent = parent;
            self.msgs.push(m);
            parent = Some(self.msgs.len() - 1);
        }
        self.active_leaf = self.msgs.len().checked_sub(1);
        self.branch_from = None;
        self.autosave();
    }

    /// Self-reflection harness (env-gated): seed a small branching conversation and optionally open
    /// the spatial view, so the GPU scene can be rendered headless without a model or user input.
    /// True when there's no conversation loaded (used to decide whether glass mode should auto-seed a demo).
    pub fn is_empty(&self) -> bool { self.msgs.is_empty() }

    /// Force the right sidebar open. For render-verification harnesses only (e.g. panels-view) —
    /// normal flow toggles `show_context` from the "☰ Panels" button or the diag-notice jump.
    pub fn debug_force_show_context(&mut self, on: bool) { self.show_context = on; }
    /// Cycle the right sidebar's subpage for render-verification harnesses. 0=Files 1=Graph
    /// 2=Context 3=Prompting 4=Engine (mirrors RightPage's declaration order; RightPage itself is
    /// private, so this takes an index rather than exposing the enum).
    pub fn debug_force_right_page(&mut self, idx: u8) {
        self.right_page = match idx { 0 => RightPage::Files, 1 => RightPage::Graph, 2 => RightPage::Context, 4 => RightPage::Engine, _ => RightPage::Prompting };
    }
    /// Also render for render-verification harnesses: flips on the metrics card (checkpoints/
    /// history need private struct construction this isn't worth adding just for a screenshot).
    pub fn debug_force_engine_extras(&mut self, on: bool) { self.show_metrics = on; }
    /// Populate Checkpoints for render-verification harnesses (History needs a real on-disk Vault
    /// — vault::commit writes content-addressed blobs — which isn't worth faking here; that card
    /// is verified by code review + the identical theme::panel()/list_item() calls already
    /// confirmed elsewhere, same as noted in the commit history).
    pub fn debug_force_checkpoints(&mut self) {
        self.checkpoints.push(Checkpoint { label: "before refactor".into(), at_unix: 0, msgs: Vec::new() });
        self.show_checkpoints = true;
    }

    pub fn boot_demo(&mut self, spatial: bool) {
        use Role::{Assistant, User};
        let turns = vec![
            (User, "create a rust crate that tails a log file".to_string()),
            (Assistant, "Here's a starting point using std::io and a seek cursor.".to_string()),
            (User, "how does it handle rotation?".to_string()),
            (Assistant, "It watches the inode; on change it reopens the path.".to_string()),
            (User, "how can we fix the truncation case?".to_string()),
            (Assistant, "Track the offset and reset to 0 when the file shrinks.".to_string()),
            (User, "what does `let mut` buy us here?".to_string()),
            (Assistant, "Mutability for the seek cursor carried across reads.".to_string()),
            (User, "why stream rather than buffer the whole file?".to_string()),
            (Assistant, "Lower latency and bounded memory on huge logs.".to_string()),
        ];
        self.load_imported(turns);
        // a few branch tips so the graph reads as a tree, not a single strand
        if self.msgs.len() >= 5 {
            let mut b = Msg::assistant(); b.variants[0] = "alt: use the notify crate for inotify events".to_string(); b.parent = Some(3); self.msgs.push(b);
            let mut c = Msg::assistant(); c.variants[0] = "alt: poll with backoff for portability".to_string(); c.parent = Some(3); self.msgs.push(c);
            let tip = self.msgs.len() - 1;
            let mut d = Msg::user("and how does this behave on Windows?".to_string()); d.parent = Some(tip); self.msgs.push(d);
        }
        self.fold_pos.clear();
        self.spatial_view = spatial;
        self.spatial_auto = true;
        self.spatial_zoomed = false;
    }

    /// Force the spatial scene on and render ONLY it (no header/composer). Used by the scene-preview
    /// fork for headless visual iteration on the themed environments.
    pub fn scene_ui(&mut self, ui: &mut egui::Ui) {
        self.spatial_view = true;
        let _ = self.spatial_ui(ui, None);
    }

    /// Render the 3D mind-palace ONLY — no spatial chrome (Overview/Prune buttons, focused-node hint), filling
    /// the whole rect — for use as a full-window background layer behind a translucent UI (glass mode).
    /// App tells the chat whether it's hosted in glass mode (so the central canvas stays transparent and the
    /// full-window backdrop draws the scene instead — avoids a double scene render).
    pub fn set_glass(&mut self, g: bool) { self.glass_host = g; }

    /// Test/verification hook: force the full-screen-spatial layout (composer + action row rendered as
    /// in-scene physical slabs instead of the 2D bottom bar) without needing to click the toggle. Used by
    /// `scene_preview`/`scene-view` so headless screenshot verification can reach those panels at all.
    pub fn set_hide_bottom_bar(&mut self, v: bool) { self.hide_bottom_bar = v; }

    /// Test/verification hook: populate the PLAN/WORKSPACE/candidates physical panels with sample data
    /// so a headless screenshot can actually reach them — none of the three has any other way to appear
    /// in `scene-view` (they're only ever filled by a live agent run), which meant they'd never been
    /// visually verified at all, unlike the dock/PARAMS panels this session already found real off-
    /// screen bugs in.
    pub fn debug_fill_agent_panels(&mut self) {
        self.agent_plan = vec![
            ("scan the workspace for entry points".to_string(), true),
            ("draft the fix in a scratch branch".to_string(), true),
            ("run the test suite against it".to_string(), false),
            ("write up findings".to_string(), false),
        ];
        self.agent_files = vec![
            crate::workspace_exec::FileEvent { path: "src/lib.rs".into(), change: crate::workspace_exec::FileChange::Modified, bytes: 1024 },
            crate::workspace_exec::FileEvent { path: "src/fix_patch.rs".into(), change: crate::workspace_exec::FileChange::Created, bytes: 512 },
        ];
        self.agent_candidates = vec![
            crate::workspace_exec::BranchSummary { index: 0, label: "conservative patch".into(), files: 2, verified: true, finished: true, approved: Some(true), doing: crate::runcard::Doing::Finished, review_notes: "clean, minimal diff".into(), root: std::path::PathBuf::new() },
            crate::workspace_exec::BranchSummary { index: 1, label: "broader refactor".into(), files: 5, verified: false, finished: true, approved: None, doing: crate::runcard::Doing::Finished, review_notes: "touches more than asked".into(), root: std::path::PathBuf::new() },
        ];
    }

    pub fn scene_backdrop(&mut self, ui: &mut egui::Ui, frost_a: f32, frost_b: f32) {
        let prev = self.spatial_view;
        self.spatial_view = true;
        self.backdrop = true;
        self.frost_a = frost_a;
        self.frost_b = frost_b;
        let _ = self.spatial_ui(ui, None);
        self.backdrop = false;
        self.spatial_view = prev;
    }

    /// Override the orbit camera for headless multi-angle capture: keep auto-targeting the graph centroid
    /// (so we always look AT the conversation) but use the supplied yaw/pitch/distance, and stop the
    /// distance auto-fit so a small `dist` puts the camera *inside* the structure.
    pub fn set_cam(&mut self, yaw: f32, pitch: f32, dist: f32) {
        self.orbit.yaw = yaw;
        self.orbit.pitch = pitch.clamp(-1.45, 1.45);
        self.orbit.dist = dist;
        self.spatial_auto = true;     // target still follows the centroid
        self.spatial_zoomed = true;   // but don't auto-fit the distance — honour `dist`
    }

    /// `set_cam` with a FIXED target: the orbit pivots on `target` instead of following the graph's
    /// centroid. For headless captures of a prop that is not at the graph's centre — the physical UI
    /// panel sits outside the graph's footprint, and an orbit around the centroid can only ever put it
    /// at the edge of frame or behind the camera.
    pub fn set_cam_target(&mut self, yaw: f32, pitch: f32, dist: f32, target: crate::spatial::Vec3) {
        self.set_cam(yaw, pitch, dist);
        self.orbit.target = target;
        self.spatial_auto = false;
    }

    /// Populate the graph from a `.logscope` script (the uploaded conversation templates): `>` lines are
    /// user turns, `:goto N` forks the next turn off main-line turn N, `:Nca`/`:Nfa`/`:fa` widen with
    /// sibling tips. Assistant turns are synthesized stubs (no model) purely so nodes read as a real tree.
    pub fn load_logscope_script(&mut self, text: &str) {
        self.msgs.clear(); self.pruned.clear(); self.queue.clear(); self.fold_pos.clear(); self.last = None;
        let mut main_line: Vec<usize> = Vec::new();   // assistant tip after each main-line user turn
        let mut tip: Option<usize> = None;            // current chain tip (assistant idx)
        let mut pending_fork: Option<usize> = None;   // set by :goto, applied to the next `>` turn
        let synth = |q: &str| -> String {
            let mut s: String = q.split_whitespace().take(9).collect::<Vec<_>>().join(" ");
            if s.len() > 64 { s.truncate(64); }
            format!("ok — {s}")
        };
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            if let Some(rest) = line.strip_prefix('>') {
                let parent = pending_fork.take().or(tip);
                let mut u = Msg::user(rest.trim().to_string()); u.parent = parent; self.msgs.push(u);
                let uid = self.msgs.len() - 1;
                let mut a = Msg::assistant(); a.variants[0] = synth(rest.trim()); a.parent = Some(uid); self.msgs.push(a);
                tip = Some(self.msgs.len() - 1);
                main_line.push(self.msgs.len() - 1);
            } else if let Some(dir) = line.strip_prefix(':') {
                let dir = dir.trim();
                if let Some(n) = dir.strip_prefix("goto").map(str::trim).and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse::<usize>().ok()) {
                    // fork the NEXT user turn off main-line turn n (clamped)
                    let idx = n.min(main_line.len().saturating_sub(1));
                    pending_fork = main_line.get(idx).copied();
                } else if dir.contains("fa") || dir.contains("ca") {
                    // widen: add up to 2 sibling assistant tips off the current turn's user parent
                    if let Some(t) = tip { if let Some(up) = self.msgs.get(t).and_then(|m| m.parent) {
                        for k in 0..2 { let mut s = Msg::assistant(); s.variants[0] = format!("alt {}", k + 1); s.parent = Some(up); self.msgs.push(s); }
                    }}
                }
                // :best / :export / others — ignored for a static populate
            }
        }
        if self.msgs.is_empty() { self.boot_demo(true); return; }
        self.active_leaf = self.msgs.len().checked_sub(1);
        self.branch_from = None;
        self.fold_pos.clear();
        self.spatial_view = true;
        self.spatial_auto = true;
        self.spatial_zoomed = false;
    }

    /// Expand one leaf: append a fresh assistant turn that continues that branch's lineage and stream into
    /// it. Drives `:continue-all` (one of these per leaf, serialized through the queue) — so the whole
    /// frontier advances. Lineage = root→leaf (active_leaf set before building the prompt), so siblings
    /// don't leak in; width comes from `:fuzzd`/reroll, this just pushes each tip forward.
    fn expand_leaf(&mut self, leaf: usize, infer: &Inference, doc: &crate::analysis::Doc) {
        if self.stream.is_some() || leaf >= self.msgs.len() { return; }
        self.active_leaf = Some(leaf);              // lineage head for the prompt builder
        let prompt = self.build_prompt(doc);        // root→leaf + the assistant primer
        // Stamp the model onto the turn it produced. convoprov marks only the CHANGES, so this is
        // what lets a conversation spanning four models read as a timeline instead of one model
        // having a bad day — which is how the template-mismatch replies were read for several turns.
        { let mut a = Msg::assistant(); a.model = self.stamp_model.clone(); self.msgs.push(a); }
        let aid = self.msgs.len() - 1;
        self.msgs[aid].parent = Some(leaf);
        self.active_leaf = Some(aid);
        self.hit_cap = false;
        self.stream = Some(Stream { idx: aid, started: Instant::now(), tokens: 0, progress: None });
        self.last = None;
        // Baseline the counters as the generation is submitted, so the panel reports this generation
        // rather than everything since process start.
        self.metrics_base = crate::metricspanel::snapshot(0);
        self.metrics_start = Some(std::time::Instant::now());
        if self.backend == BackendKind::Remote { infer.generate_chat(self.build_turns(doc)); } else { infer.generate(prompt); }
    }

    /// Build the prompt from history `msgs[..upto]` (so regenerate/edit can re-run from a point).
    /// The time line for the prompt, or nothing.
    ///
    /// ## When it is included, and why that default
    ///
    /// You asked twice how this should be wired — every turn, or only when a
    /// limit is live — and said keep working twice, so: BOTH, with a floor.
    ///
    /// * a live work limit ("keep working for the next hour") always says so,
    ///   because that is the instruction the model otherwise cannot read;
    /// * otherwise the line appears only once the session passes ten minutes.
    ///
    /// The floor is the whole argument. Prepending "Time: 40 seconds into this
    /// session, 2 turns" to every exchange spends context on every turn to say
    /// nothing, and a line that is usually noise is a line the model learns to
    /// skip — including on the turn it finally matters. Ten minutes is roughly
    /// where "how long have I been at this" stops being obvious from the
    /// transcript length.
    ///
    /// THE ALTERNATIVE I DID NOT TAKE: limit-only. It is cheaper and it drops
    /// the case your transcript actually showed — a long unbounded session
    /// where the model had no idea it had been running for an hour, which is
    /// exactly when a wall clock is worth its tokens.
    fn time_line(&self) -> Option<String> {
        let first = self.msgs.first()?;
        let now = wall_now();
        let limit = self.work_limit;
        let ctx = logscope_fan::sessiontime::TimeContext {
            started: first.at, now, turns: self.msgs.len(),
            limit,
        };
        // A clock that went backwards, or a transcript reloaded from a file
        // written last week: `started` is a stored wall time and nothing
        // guarantees it precedes `now`. Reporting "0 seconds into this session"
        // for a resumed chat is better than reporting a negative or an
        // absurdity, and `elapsed` already saturates — this just declines to
        // volunteer a figure it cannot stand behind.
        if limit.is_none() && ctx.elapsed().as_secs() < 600 { return None }
        Some(ctx.render())
    }

    fn build_prompt_upto(&self, upto: usize, doc: &crate::analysis::Doc) -> String {
        let mut sys = neutralize_markers(&self.full_system(doc));
        if self.stepwise { sys.push_str(crate::stepwise::STEPWISE_PROMPT); }
        // The liaison persona is written for a turn that HAS observations. Selected as a chat
        // persona most turns have none — which is why every reply opened with a backend status line,
        // including the answer to "hi there", and why "tell the debaters..." produced an error code
        // for a run that never started.
        if sys.contains("You are a liaison") {
            sys.push_str("\n\n");
            sys.push_str(logscope::liaison_prompt::LIAISON_NO_OBSERVATIONS);
        }
        if self.patch_mode { sys.push_str(crate::patchmode::PATCH_MODE_PROMPT); }
        // The clock, last so it sits closest to the conversation.
        if let Some(t) = self.time_line() { sys.push_str("\n\n"); sys.push_str(&t); }
        let sys = sys;
        match self.backend {
            // Read the template LIVE rather than trusting the per-frame copy. The copy is refreshed in
            // the update loop, so a generate submitted in the same frame as a model load still carried
            // the PREVIOUS model's template — which is the "stale UI state" the worker kept reporting.
            BackendKind::Wgpu => if let Some(p) = self.build_prompt_gguf(&sys, upto) { p } else { match self.live_template() {
                TemplateKind::ChatML => self.build_prompt_chatml(&sys, upto),
                TemplateKind::Llama3 => self.build_prompt_llama3(&sys, upto),
                // Llama4: build with Llama-3 markers — the worker's normalize_prompt_template
                // rewrites them to <|header_start|>/<|eot|> authoritatively at generate time.
                TemplateKind::Llama4 => self.build_prompt_llama3(&sys, upto),
                TemplateKind::Gemma => self.build_prompt_gemma(&sys, upto),
                TemplateKind::Bailing2 => self.build_prompt_bailing2(&sys, upto),
                TemplateKind::Plain => self.build_prompt_plain(&sys, upto),
            } },
            // Remote templates server-side (generate_chat); this prompt is unused for it, so keep it Plain.
            BackendKind::Remote => self.build_prompt_plain(&sys, upto),
            // llama.cpp: prefer the model's own GGUF template when the flag is on (same as the wgpu arm),
            // else Plain — but Gemma needs its own turn markers to stop correctly.
            _ => self.build_prompt_gguf(&sys, upto).unwrap_or_else(||
                if self.live_template() == TemplateKind::Gemma { self.build_prompt_gemma(&sys, upto) } else { self.build_prompt_plain(&sys, upto) }),
        }
    }

    /// Prompt to CONTINUE the last (partial) assistant reply: the same context the turn had, then
    /// the assistant turn left OPEN with the partial text and no closing marker, so the model picks
    /// up exactly where it was cut off rather than starting a new turn. Streaming appends straight
    /// into the same message. Uses the turns BEFORE the partial (chosen_msgs(.., last)) so the user
    /// question is still present even in single-shot mode.
    fn build_continue_prompt(&self, doc: &crate::analysis::Doc) -> String {
        let mut sys = neutralize_markers(&self.full_system(doc));
        if self.stepwise { sys.push_str(crate::stepwise::STEPWISE_PROMPT); }
        // The liaison persona is written for a turn that HAS observations. Selected as a chat
        // persona most turns have none — which is why every reply opened with a backend status line,
        // including the answer to "hi there", and why "tell the debaters..." produced an error code
        // for a run that never started.
        if sys.contains("You are a liaison") {
            sys.push_str("\n\n");
            sys.push_str(logscope::liaison_prompt::LIAISON_NO_OBSERVATIONS);
        }
        if self.patch_mode { sys.push_str(crate::patchmode::PATCH_MODE_PROMPT); }
        // The clock, last so it sits closest to the conversation.
        if let Some(t) = self.time_line() { sys.push_str("\n\n"); sys.push_str(&t); }
        let sys = sys;
        let last = self.msgs.len().saturating_sub(1);
        let raw_partial = self.msgs.get(last).map(|m| m.text().to_string()).unwrap_or_default();
        // If the reply ended by asking to continue, drop that trailing question before resuming IN PLACE —
        // otherwise the model reads its own "let me know when you're ready" and stalls or answers itself.
        let partial = neutralize_markers(crate::stepwise::strip_trailing_solicitation(&raw_partial));
        // RESUME BUILDER TEMPLATE SELECTION. Two defects fixed here, both visible in the 737
        // hardware log, which showed a Bailing2-DETECTED model handed a prompt ending
        // "assistant<|end_header_id|>\n\n" with ZERO special tokens:
        //
        // 1. It read `self.template` (the tab's persisted value) instead of `live_template()`,
        //    which prefers what the worker actually detected for the loaded model. A restored
        //    conversation carrying a stale Llama3 choice therefore framed every continuation in
        //    Llama3 markers no matter which model was loaded.
        // 2. It knew only ChatML and Llama3/Llama4; every other family fell through to bare
        //    "System:/User:" text. Bailing2 (Ling 2.0 / LLaDA2) had no arm at all, so even with
        //    correct detection this path could never emit <role> turns.
        //
        // The markers of a family the tokenizer does not know go in as ordinary words — which is
        // exactly what "0 special tokens" on a 1718-token prompt meant.
        let t_live = self.live_template();
        let chatml = self.backend == BackendKind::Wgpu && t_live == TemplateKind::ChatML;
        let llama3 = self.backend == BackendKind::Wgpu && matches!(t_live, TemplateKind::Llama3 | TemplateKind::Llama4);
        let bailing2 = self.backend == BackendKind::Wgpu && t_live == TemplateKind::Bailing2;
        if bailing2 {
            // Same frame as build_prompt_bailing2, with the assistant turn left OPEN so the partial
            // resumes in place: no <|role_end|> after it.
            let nb = |t: &str| neutralize_markers(t).replace("<role>", "<\u{200b}role>").replace("</role>", "<\u{200b}/role>");
            let mut s = if sys.trim().is_empty() {
                String::from("<role>SYSTEM</role>detailed thinking off<|role_end|>")
            } else {
                format!("<role>SYSTEM</role>{}\ndetailed thinking off<|role_end|>", nb(&sys))
            };
            for m in self.chosen_msgs(8, &sys, last) {
                let role = if m.role == Role::User { "HUMAN" } else { "ASSISTANT" };
                s.push_str(&format!("<role>{role}</role>{}<|role_end|>", nb(m.text())));
            }
            s.push_str(&format!("<role>ASSISTANT</role>{partial}"));
            s
        } else if chatml {
            let mut s = format!("<|im_start|>system\n{}<|im_end|>\n", sys);
            for m in self.chosen_msgs(8, &sys, last) {
                let role = if m.role == Role::User { "user" } else { "assistant" };
                s.push_str(&format!("<|im_start|>{role}\n{}<|im_end|>\n", neutralize_markers(m.text())));
            }
            // Open the assistant turn with the partial and NO closing marker → it resumes in place.
            s.push_str(&format!("<|im_start|>assistant\n{}", partial));
            s
        } else if llama3 {
            let mut s = format!("<|begin_of_text|><|start_header_id|>system<|end_header_id|>\n\n{}<|eot_id|>", sys);
            for m in self.chosen_msgs(8, &sys, last) {
                let role = if m.role == Role::User { "user" } else { "assistant" };
                s.push_str(&format!("<|start_header_id|>{role}<|end_header_id|>\n\n{}<|eot_id|>", neutralize_markers(m.text())));
            }
            s.push_str(&format!("<|start_header_id|>assistant<|end_header_id|>\n\n{}", partial));
            s
        } else {
            // NO RESUME ARM for this template: the continuation is framed as bare text while the
            // model expects its own turn markers. Silent before — say it once, because the symptom
            // (a coherent first reply, degraded continuations) points nowhere near this code.
            if self.backend == BackendKind::Wgpu && t_live != TemplateKind::Plain {
                use std::sync::atomic::{AtomicBool, Ordering};
                static SAID: AtomicBool = AtomicBool::new(false);
                if !SAID.swap(true, Ordering::Relaxed) {
                    eprintln!("logscope: continuation prompt has no arm for template {t_live:?} — \
framing it as plain 'System:/User:' text. The model's own turn markers are absent, so a resumed \
reply is framed differently from the turn that produced it.");
                }
            }
            let mut s = format!("System: {}\n", sys);
            for m in self.chosen_msgs(2, &sys, last) {
                let tag = if m.role == Role::User { "User" } else { "Assistant" };
                s.push_str(&format!("{tag}: {}\n", neutralize_markers(m.text())));
            }
            s.push_str(&format!("Assistant: {}", partial));
            s
        }
    }

    /// Resume the last reply in place: stream more tokens into the same assistant message using a
    /// continuation prompt. No new bubble — the reply just grows.
    fn continue_generation(&mut self, infer: &Inference, doc: &crate::analysis::Doc) {
        if self.stream.is_some() { return; }
        let last = self.msgs.len().saturating_sub(1);
        match self.msgs.get(last) {
            Some(m) if m.role == Role::Assistant && !m.text().is_empty() => {}
            _ => return,
        }
        let fallback = self.build_continue_prompt(doc); // used only if the engine can't warm-resume
        self.hit_cap = false;
        self.step_was_continue = true; // Done handler uses this to catch a 0-token (marker-only) resume
        self.stream = Some(Stream { idx: last, started: Instant::now(), tokens: 0, progress: None });
        self.last = None;
        infer.continue_stream(fallback);
    }

    // ── persistence / checkpoints / recovery ──
    /// Write the live conversation + checkpoints to disk (best-effort). Called on each completed
    /// turn and on checkpoint changes, so a crash mid-generation recovers everything.
    fn autosave(&self) {
        let Some(path) = autosave_path() else { return };
        let save = ChatSave { msgs: self.msgs.iter().map(msg_to_save).collect(),
                              checkpoints: self.checkpoints.clone(),
                              replicator: self.replicator.as_ref().map(|m| m.snapshot()) };
        if let Ok(json) = serde_json::to_string(&save) {
            if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
            let _ = std::fs::write(&path, json);
        }
    }
    fn load_autosave() -> Option<ChatSave> {
        let path = autosave_path()?;
        serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()
    }
    /// Snapshot the conversation as a restorable checkpoint (bounded; oldest auto-pruned).
    fn checkpoint(&mut self, label: impl Into<String>) {
        if self.msgs.is_empty() { return; }
        self.checkpoints.push(Checkpoint { label: label.into(), at_unix: now_unix(), msgs: self.msgs.iter().map(msg_to_save).collect() });
        let n = self.checkpoints.len();
        if n > MAX_CHECKPOINTS { self.checkpoints.drain(0..n - MAX_CHECKPOINTS); }
        self.autosave();
    }
    /// Roll back to a checkpoint. Non-destructive: snapshots the CURRENT state first ("before
    /// rollback") so a rollback is itself reversible.
    fn rollback_to(&mut self, i: usize) {
        if self.stream.is_some() || i >= self.checkpoints.len() { return; }
        let cp = self.checkpoints[i].clone();
        if !self.msgs.is_empty() { self.checkpoint("before rollback"); }
        self.msgs = cp.msgs.iter().map(save_to_msg).collect(); self.resync_tree();
        self.hit_cap = false; self.last = None;
        self.autosave();
    }
    /// Startup-only: if there's an autosave and no live conversation, offer to recover it.
    fn check_restore(&mut self) {
        if self.checked_restore { return; }
        self.checked_restore = true;
        if !self.msgs.is_empty() { return; }
        if let Some(save) = Self::load_autosave() { if !save.msgs.is_empty() { self.recover = Some(save.msgs.len()); } }
    }
    fn restore_autosave(&mut self) {
        if let Some(save) = Self::load_autosave() {
            self.msgs = save.msgs.iter().map(save_to_msg).collect(); self.resync_tree();
            self.checkpoints = save.checkpoints;
            // Reopen the fan the operator left running: its assignments and cadences, at the
            // column count it was saved with, bounded by what this machine can hold now (the save
            // may come from a bigger one). Transcripts come back through the conversation above,
            // not through the fan — one owner for message history.
            if let Some(rs) = save.replicator.filter(|r| !r.assignments.is_empty()) {
                let hosts = self.replicator_hosts();
                let slots = logscope::replicator::slots(&hosts, self.instance_bytes);
                let n = rs.assignments.len().clamp(slots.min, slots.selectable_max());
                let mut m = ReplicatorMode::new(&slots, n);
                m.restore(&rs, n);
                self.note = format!("replicator: reopened {n} worker(s) from the last session");
                self.replicator = Some(m);
            }
        }
        self.recover = None;
    }

    /// The conversation as portable Markdown (You / Assistant sections).
    fn transcript(&self) -> String {
        let mut s = String::from("# logscope chat\n\n");
        // WHEN, and HOW LONG SINCE THE LAST TURN.
        //
        // The export carried no time at all, so a conversation reopened a week
        // later read exactly like one from a minute ago — and an operator who
        // typed "keep working for the next hour" left no trace of when the hour
        // started. Both halves matter and the second more: absolute stamps tell
        // you when, gaps tell you what happened between turns, and a
        // twenty-minute gap before a one-line reply is the shape of a model
        // that stalled.
        //
        // Gaps only when they are worth a reader's attention. A stamp on every
        // turn of a fast exchange is noise, and noise in a header is what makes
        // people stop reading headers.
        if let Some(first) = self.msgs.first() {
            s.push_str(&format!("Started {}\n\n", stamp_utc(first.at)));
        }
        let mut prev: Option<u64> = None;
        for m in &self.msgs {
            s.push_str(if m.role == Role::User { "## You" } else { "## Assistant" });
            // BOTH ENDS must be real. A turn written before `MsgSave.at`
            // existed loads with 0, and `m.at - 0` is the whole unix epoch —
            // a part-migrated autosave would report "(473000 hours later)"
            // between its last old turn and its first new one. That is the
            // ordinary case for anyone who had a chat open when they updated.
            let gap = match (prev, m.at) {
                (Some(p), now) if p > 0 && now > 0 => now.saturating_sub(p),
                _ => 0,
            };
            if gap >= 60 {
                s.push_str(&format!("  ({} later, {})",
                    logscope_fan::sessiontime::human(std::time::Duration::from_secs(gap)),
                    stamp_utc(m.at)));
            }
            s.push_str("\n\n");
            s.push_str(m.text());
            s.push_str("\n\n");
            prev = Some(m.at);   // 0 stays 0; the match above treats it as absent
        }
        s
    }
    /// Copy the transcript to the clipboard and also write a timestamped Markdown file next to the
    /// autosave; returns the file path (if written) for a confirmation toast.
    fn export_chat(&self, ctx: &egui::Context) -> Option<std::path::PathBuf> {
        let text = self.transcript();
        ctx.copy_text(text.clone());
        let dir = autosave_path()?.parent()?.join("exports");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("chat-{}.md", now_unix()));
        std::fs::write(&path, text).ok()?;
        Some(path)
    }

    /// Rough token estimate for the next prompt (system + threaded history + composer).
    fn est_prompt_tokens(&self, doc: &crate::analysis::Doc) -> usize {
        let sys = est(&self.full_system(doc));
        let window = if self.threading { self.ctx.history_turns * 2 + 1 } else { 1 };
        let hist: usize = self.msgs.iter().rev().take(window).map(|m| est(m.text()) + 8).sum();
        sys + hist + est(&self.input) + 8
    }
    /// A larger context window to suggest if the next prompt would overflow the current one (so the
    /// backend would trim it). Rounded to a 1k step, capped at the ceiling.
    fn ctx_suggestion(&self, doc: &crate::analysis::Doc) -> Option<u32> {
        let need = self.est_prompt_tokens(doc) + self.ctx.max_tokens as usize;
        if need <= self.ctx.n_ctx as usize { return None; }
        let want = (need.div_ceil(1024) * 1024).min(32768) as u32;
        (want > self.ctx.n_ctx).then_some(want)
    }

    /// System prompt actually sent: the base/edited context, plus a bounded summary of the
    /// current log when "attach log" is on. Giving a small model the real log up front —
    /// line count, the most-repeated line patterns, the last few lines — turns vague
    /// guesses into specific, grounded answers without dumping the whole (possibly huge) file.
    fn full_system(&self, doc: &crate::analysis::Doc) -> String {
        let mut s = self.sys().to_string();
        if let Some(b) = self.log_block(doc) { s.push_str("\n\n"); s.push_str(&b); }
        // THE WORKSPACE, IN A PLAIN CHAT TURN. `:ws` mounted a repository and
        // nothing in this prompt said so: the field transcript is a model
        // answering "we would need to see the full definition of
        // guidance::Target" while that definition sat one read away. The agent
        // path had context all along (agent_context); a chat turn did not, and
        // a chat turn is what people actually type into.
        //
        // Budgeted by the same class as everywhere else, so a 1B gets a short
        // tree and a nudge, not a repository dump — the point is that it KNOWS
        // and can ask, not that it reads everything up front.
        if let Some(b) = self.workspace_block() { s.push_str("\n\n"); s.push_str(&b); }
        if self.fix_mode { s.push_str("\n\n"); s.push_str(FIX_MODE); }
        // Task posture: classify the latest user turn so a maintenance-tuned identity prompt doesn't refuse
        // a legitimate greenfield build. The posture reframes minimal-edit as "build the smallest working
        // version, then grow it" for new-build requests, and keeps the literal minimal-edit posture for
        // maintenance — humility channeled into HOW to build, not WHETHER to. (Only the long-horizon
        // quadrants add text; one-shot asks get nothing.)
        if let Some(latest) = self.msgs.iter().rev().find(|m| m.role == Role::User) {
            let class = crate::taskclass::classify(latest.text());
            if let Some(p) = crate::taskclass::posture(class) { s.push_str(p); }
            // Educate-then-defer: for substantive tasks, inform once then respect an informed directive
            // (the antidote to the re-litigation loop). Applied to long-horizon work where there's actually
            // something worth flagging.
            if class.wants_orchestration() { s.push_str(crate::taskclass::EDUCATE_THEN_DEFER); }
            // If the user reaffirmed a request after the model pushed back, tell the model to proceed now
            // rather than decline again — the signal that would have unstuck the refused transcript.
            if let Some(prev) = self.msgs.iter().rev().filter(|m| m.role == Role::Assistant).find(|m| !m.text().is_empty()) {
                if crate::taskclass::reaffirmed_after_pushback(prev.text(), latest.text()) {
                    s.push_str(crate::taskclass::REAFFIRMED_PROCEED);
                }
            }
        }
        s
    }

    fn log_block(&self, doc: &crate::analysis::Doc) -> Option<String> {
        // Never attach the BUILT-IN demo document. Attaching it tells the model to answer from a file
        // the user has never seen, with "use only this data" — which is why a greeting came back as an
        // analysis of sample.log. A real load clears the flag.
        if !self.attach_log || doc.lines.is_empty() || doc.placeholder { return None; }
        let name = if doc.name.is_empty() { "(unnamed)" } else { &doc.name };
        let mut s = format!("CURRENT LOG: {name} ({} lines). Use only this data; cite line patterns or lines from it.", doc.lines.len());
        let tops = crate::analysis::repeated_templates(&doc.lines, 5);
        if !tops.is_empty() {
            s.push_str("\nMost-repeated line patterns:");
            for (t, c) in tops.iter().take(5) {
                let t: String = t.chars().take(120).collect();
                s.push_str(&format!("\n  {c}x  {t}"));
            }
        }
        let tail: Vec<&String> = doc.lines.iter().rev().take(6).collect();
        if !tail.is_empty() {
            s.push_str("\nLast lines:");
            for l in tail.iter().rev() {
                let l: String = l.chars().take(160).collect();
                s.push_str(&format!("\n  {l}"));
            }
        }
        Some(s)
    }

    /// The conversation as a turn-graph spine up to `upto` (the durable structure the Graph view and
    /// the prompt builder both read). Node ids are message indices.
    /// After (re)loading the flat msg list, re-seat the live branch tip and clear transient branch state.
    fn resync_tree(&mut self) { self.active_leaf = self.msgs.len().checked_sub(1); self.branch_from = None; self.queue.clear(); self.fan = None; self.fold_pos.clear(); self.pruned.retain(|&p| p < self.msgs.len()); }

    fn convo_graph(&self, upto: usize) -> crate::convograph::Graph {
        let n = upto.min(self.msgs.len());
        // a turn's parent is its recorded parent when set, else the previous turn (linear fallback for
        // pre-branch conversations) — so the live branch is whatever lineage `active_leaf` sits on.
        let head = self.active_leaf.filter(|&h| h < n);
        let mut g = crate::convograph::Graph::from_tree(
            self.msgs[..n].iter().enumerate().map(|(i, m)| (
                if m.role == Role::User { crate::convograph::Turn::User } else { crate::convograph::Turn::Assistant },
                m.text(),
                m.parent.or(if i > 0 { Some(i - 1) } else { None }))),
            est, head);
        if !self.pruned.is_empty() { for nd in &mut g.nodes { nd.pruned = self.pruned.contains(&nd.id); } }
        // ARTIFACTS produced by this conversation, hung off the last turn. They are appended AFTER the
        // pruned marking so a pruned turn does not accidentally prune the file it produced — the file
        // still exists on disk regardless of whether its turn is still in context.
        //
        // Deduplicated by path, keeping the LAST event: a file written three times is one artifact with
        // its final size, not three nodes, and a graph that showed three would misrepresent the workspace.
        if !self.agent_files.is_empty() {
            let mut seen: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
            let mut order: Vec<&str> = Vec::new();
            for f in &self.agent_files {
                if seen.insert(f.path.as_str(), f.bytes).is_none() { order.push(f.path.as_str()); }
                else { seen.insert(f.path.as_str(), f.bytes); }
            }
            for path in order {
                let bytes = seen.get(path).copied().unwrap_or(0);
                g.push_file(path, bytes, None, est);
            }
        }
        g
    }

    /// Most-recent turns that fit the budget — the in-context slice of the conversation graph, i.e. the
    /// SAME selection the Graph sidebar paints. convograph is the single authority; this is what the
    /// model is actually fed. When threading is off, only the latest message is kept (context leads).
    fn chosen_msgs(&self, per_msg_overhead: usize, sys: &str, upto: usize) -> Vec<&Msg> {
        let budget = self.ctx.n_ctx.saturating_sub(self.ctx.max_tokens) as usize;
        let max_msgs = if self.threading { self.ctx.history_turns * 2 + 1 } else { 1 };
        let mut g = self.convo_graph(upto);
        // LOGSCOPE_WINDOW_SLACK=0 restores the plain tail window exactly; the default holds the floor
        // and, when it must move, jumps far enough to sit still for several turns. The trade is
        // fewer turns in context right after a jump, in exchange for a prompt prefix the KV tree can
        // actually match. Simulated over 40 turns: 30+ prefix changes with the tail window against 8
        // at this slack.
        let slack: f32 = std::env::var("LOGSCOPE_WINDOW_SLACK").ok()
            .and_then(|v| v.trim().parse().ok()).filter(|v: &f32| (0.0..=0.9).contains(v))
            .unwrap_or(0.35);
        let (_, _, new_floor) = g.mark_window_sticky(
            budget, est(sys) + 8, per_msg_overhead, max_msgs, self.win_floor.get(), slack);
        if upto == self.msgs.len() { self.win_floor.set(new_floor); }
        g.context_ids().into_iter().map(|id| &self.msgs[id]).collect()
    }

    fn sys(&self) -> &str {
        let s = self.context.trim();
        if s.is_empty() { LONG_HORIZON } else { s }
    }

    /// Plain transcript format (llama.cpp path; the model adds BOS).
    fn build_prompt_plain(&self, sys: &str, upto: usize) -> String {
        let mut s = format!("System: {}\n", sys);
        for m in self.chosen_msgs(2, sys, upto) {
            let tag = if m.role == Role::User { "User" } else { "Assistant" };
            s.push_str(&format!("{tag}: {}\n", neutralize_markers(m.text())));
        }
        s.push_str("Assistant:");
        s
    }

    /// Llama-3 instruct chat template; the wgpu tokenizer maps the markers to control
    /// tokens and the engine stops on the <|eot_id|>/eog set.
    fn build_prompt_llama3(&self, sys: &str, upto: usize) -> String {
        let mut s = format!("<|begin_of_text|><|start_header_id|>system<|end_header_id|>\n\n{}<|eot_id|>", sys);
        for m in self.chosen_msgs(8, sys, upto) {
            let role = if m.role == Role::User { "user" } else { "assistant" };
            s.push_str(&format!("<|start_header_id|>{role}<|end_header_id|>\n\n{}<|eot_id|>", neutralize_markers(m.text())));
        }
        s.push_str("<|start_header_id|>assistant<|end_header_id|>\n\n");
        s
    }

    /// ChatML template (smollm2 and other `<|im_end|>` models). Feeding these the Llama-3 markers
    /// above makes the tokenizer emit them as byte-level text — the model never reaches its real
    /// stop (`<|im_end|>`) and runs to the cap. Matching the template is the fix.
    /// The active model's own template, from its GGUF, rendered with the current history. Returns None
    /// when the flag is off, no model is loaded, the GGUF carries no template, or rendering fails —
    /// every one of which falls back to the hand-built family. Cached per model path.
    fn build_prompt_gguf(&self, sys: &str, upto: usize) -> Option<String> {
        if std::env::var("LOGSCOPE_GGUF_TEMPLATE").as_deref() != Ok("1") { return None; }
        let path = logscope::llm_backend::active_model()?.path.clone();
        let stale = self.gguf_tmpl.borrow().as_ref().map(|(p, _)| p != &path).unwrap_or(true);
        if stale {
            let r = template_kit::gguf::read_templates_from(std::path::Path::new(&path)).ok()
                .and_then(|t| t.for_tools().map(|s| s.to_string()))
                .and_then(|src| template_kit::Renderer::new(&src).ok())
                .map(std::sync::Arc::new);
            *self.gguf_tmpl.borrow_mut() = Some((path.clone(), r));
        }
        let renderer = self.gguf_tmpl.borrow().as_ref().and_then(|(_, r)| r.clone())?;
        let mut msgs = Vec::new();
        if !sys.trim().is_empty() { msgs.push(template_kit::Message::system(sys)); }
        for m in self.chosen_msgs(8, sys, upto) {
            let text = neutralize_markers(m.text());
            msgs.push(if m.role == Role::User { template_kit::Message::user(&text) } else { template_kit::Message::assistant(&text) });
        }
        match renderer.render(&msgs, &[], true) { Ok(p) => Some(p), Err(_) => None }
    }

    fn build_prompt_chatml(&self, sys: &str, upto: usize) -> String {
        let mut s = format!("<|im_start|>system\n{}<|im_end|>\n", sys);
        for m in self.chosen_msgs(8, sys, upto) {
            let role = if m.role == Role::User { "user" } else { "assistant" };
            s.push_str(&format!("<|im_start|>{role}\n{}<|im_end|>\n", neutralize_markers(m.text())));
        }
        s.push_str("<|im_start|>assistant\n");
        s
    }

    /// Gemma 4 chat template. Gemma 4 CHANGED its turn markers from Gemma 3: it uses `<|turn>ROLE\n…<turn|>`
    /// (not `<start_of_turn>`/`<end_of_turn>`) and adds a native `system` role. This is the BASIC-CHAT form —
    /// Gemma 4's tool tokens (`<|tool|>` …) and thinking channels (`<|think|>`, `<|channel>`) are not emitted
    /// here. BOS is left to the backend (llama.cpp adds `<bos>`). The authoritative template ships as the
    /// GGUF's embedded jinja; if this ever drifts, prefer having the backend apply that instead.
    fn build_prompt_gemma(&self, sys: &str, upto: usize) -> String {
        let mut s = String::new();
        if !sys.trim().is_empty() { s.push_str(&format!("<|turn>system\n{}<turn|>\n", sys)); }
        for m in self.chosen_msgs(8, sys, upto) {
            let role = if m.role == Role::User { "user" } else { "model" };
            s.push_str(&format!("<|turn>{role}\n{}<turn|>\n", neutralize_markers(m.text())));
        }
        s.push_str("<|turn>model\n");
        s
    }

    fn build_prompt_bailing2(&self, sys: &str, upto: usize) -> String {
        // Bailing2 (Ling 2.0 / LLaDA2), mirrored from llama.cpp llama-chat.cpp: uppercase roles in
        // <role>...</role>, turns closed by <|role_end|>, and — faithful to the reference — a
        // DEFAULT system of "detailed thinking off" when none is given (the thinking toggle lives
        // in the system turn, so omitting it is not neutral). Content gets the standard marker
        // neutralization PLUS a break on plain-bracket <role>/</role>, which the <| rule misses.
        let nb = |t: &str| neutralize_markers(t).replace("<role>", "<\u{200b}role>").replace("</role>", "<\u{200b}/role>");
        let mut s = String::new();
        if sys.trim().is_empty() { s.push_str("<role>SYSTEM</role>detailed thinking off<|role_end|>"); }
        else { s.push_str(&format!("<role>SYSTEM</role>{}\ndetailed thinking off<|role_end|>", nb(sys))); }
        for m in self.chosen_msgs(8, sys, upto) {
            let role = if m.role == Role::User { "HUMAN" } else { "ASSISTANT" };
            s.push_str(&format!("<role>{role}</role>{}<|role_end|>", nb(m.text())));
        }
        s.push_str("<role>ASSISTANT</role>");
        s
    }

    fn send(&mut self, infer: &Inference, doc: &crate::analysis::Doc, ui: &egui::Ui) {
        // Composer command routing: a line the user submits that begins with ':' is executed as a
        // command HERE, not sent to the model. '::text' escapes a literal message that starts with a
        // colon. Everything else is a normal chat turn.
        // '::' before compose: the escape hatch OUT of the workspace auto-route below — a message
        // the user explicitly wants as plain chat even with a repository loaded.
        let plain_escape = self.input.trim_start().starts_with("::");
        match compose_input(&self.input) {
            Composed::Empty => {}
            Composed::Command(cmd) => {
                self.input.clear();
                match cmd {
                    crate::vimmode::Command::Noop => {}
                    crate::vimmode::Command::Unknown(l) =>
                        self.note = format!("unknown command '{}' — :help to list commands", l.trim().trim_start_matches(':')),
                    other => self.run_command(other, ui),
                }
            }
            Composed::Message(qt) => {
                self.input.clear();
                // REPLICATOR MODE OWNS A PLAIN MESSAGE. In this mode the composer is not talking to
                // one model; it is addressing a fan (broadcast) or one column (aimed). Falling
                // through to the single-model path here would send the message to a worker the
                // operator did not choose and leave the columns silent — the mode would look
                // broken while quietly working normally underneath.
                if self.replicator.is_some() && !plain_escape {
                    self.dispatch_replicator(&qt);
                    return;
                }
                // WORKSPACE AUTO-ROUTE: with a repository loaded, a plain message IS a task on that
                // repository — run it through the agent loop (which can read/search/edit the code)
                // instead of plain chat (which cannot; the log's "add more blocks" got a generic
                // clarification reply because the model never saw a single file). `::text` still
                // escapes a literal chat message, and a busy model falls through to the normal queue.
                if !plain_escape && self.gitmind_session.is_some() && self.agent_run.is_none() && self.agent_fan.is_none() && self.stream.is_none() {
                    self.start_agent_task(qt, "task");
                    if self.agent_run.is_some() {
                        self.note = format!("{} — auto-routed to the workspace agent ('::' prefix for plain chat)", self.note);
                    }
                    return;
                }
                // Branch point: a human interjection forks off the selected node (or the previous tip).
                let parent = self.branch_from.take().or(self.active_leaf).or(self.msgs.len().checked_sub(1));
                // Fire-and-queue: if the model is busy, don't stop it — enqueue this (branch) turn to run as
                // soon as the current reply finishes. This is what makes branching non-blocking. An active
                // agent run owns the model too, so queue behind it rather than colliding.
                if self.stream.is_some() || self.agent_run.is_some() || self.agent_fan.is_some() { self.queue.push_back((qt, parent)); return; }
                self.start_generation(qt, parent, infer, doc);
            }
        }
    }

    /// The base text a composer-invoked fan command (`:fuzz`/`:fuzzd`) works from: the composer if it holds
    /// a prompt (the two-box cmdline workflow), else the last user turn — so `:fuzz N` typed straight into
    /// the message box means "fan N retries of my last prompt", matching the command's documented intent.
    fn base_for_fan(&self) -> String {
        let c = self.input.trim();
        if !c.is_empty() { return c.to_string(); }
        self.msgs.iter().rev().find(|m| m.role == Role::User).map(|m| m.text().to_string()).unwrap_or_default()
    }

    /// Actually begin a generation: checkpoint, append the user turn (forked at `parent`) + an empty
    /// assistant turn, build the prompt for the live branch, and stream. Shared by send + the queue.
    /// Clear the conversation: wipe the transcript/tree AND the per-conversation generation bookkeeping
    /// (stepwise checkpoint counters, in-flight queue/fan, live estimate samples). The Clear button used to
    /// reset only the transcript, so the ⏸ STEP counters and generation estimate carried into the next
    /// conversation — this centralizes the reset so the two can't drift apart again.
    fn clear_conversation(&mut self) {
        self.msgs.clear();
        self.last = None;
        self.active_leaf = None;
        self.branch_from = None;
        self.pruned.clear();
        self.queue.clear();
        self.fold_pos.clear();
        self.region = None;
        self.fan = None;
        self.count_pending = None;
        // stepwise/generation state that was leaking across a clear
        self.stepwise = false;
        self.step_last_paused = 0;
        self.step_continues = 0;
        self.step_pending_continue = false;
        self.step_arm_continue = false;
        self.step_shell = None;
        self.step_inject = None;
        self.hit_cap = false;
        self.gen_samples.clear();
        self.last_prompt_tokens = 0;
    }

    fn start_generation(&mut self, qt: String, parent: Option<usize>, infer: &Inference, doc: &crate::analysis::Doc) {
        // ROUTE THE PROMPT before generating.
        //
        // Every turn used to go to the same model with the same
        // permissions, which is why `liaison`'s epistemic discipline
        // applied to nothing: the HUD was fed engine telemetry and never
        // a statement carrying a kind. The matrix decides destination and
        // CEILING; the ceiling is what stops a model asserting the
        // runtime's own facts.
        let route = crate::promptroute::route(
            crate::promptroute::classify(&qt),
            crate::promptroute::Available {
                // `detected_template()` is Some only after a successful
                // load — the worker publishes it and 255 means nothing
                // loaded. Using it rather than a cached per-tab flag for
                // the same reason the prompt builder does: a tab created
                // after the load would otherwise think no model exists.
                engine: infer.detected_template().is_some(),
                // The roster landed, so this is live: a Judgement prompt
                // now reaches a generated persona when one exists, and
                // falls back to the plain engine when the roster is
                // empty — which it is until someone supplies a corpus.
                workers: !self.workers.is_empty(),
                runtime: true,
            },
        );
        // The operator's own words, recorded as what they are. This is
        // the statement kind the HUD never got: it only ever saw
        // telemetry, so nothing on screen distinguished "you said this"
        // from "the engine measured this".
        self.hud.observe(qt.chars().take(80).collect::<String>(), "operator");
        if route.dest == crate::promptroute::Dest::Nowhere {
            // Refuse rather than answer. A question about the run with
            // nothing able to answer it must not fall through to a model
            // that will answer anyway — that is the failure the matrix
            // exists to make unreachable, and it is worth the same
            // handling here as in the table.
            self.note = "nothing loaded can answer that — load a model, or ask about the run once \
                         a run has happened"
                .to_string();
            return;
        }
        self.route_ceiling = Some(route.ceiling);
        // THE PERSONA, when the prompt routed to one. Without this the
        // route would name a destination and change nothing about the
        // request — a worker that is chosen and never spoken through.
        //
        // `system_prompt` returns None on an empty roster, so this is
        // also the fallback: no persona means the prompt goes to the
        // engine plain, which is what `Dest::Engine` would have done.
        self.route_persona = if route.dest == crate::promptroute::Dest::Worker {
            self.workers.system_prompt(&qt)
        } else {
            None
        };

        // fresh reply → fresh stepwise accounting
        self.step_last_paused = 0; self.step_continues = 0; self.step_pending_continue = false; self.step_arm_continue = false;
        if !self.msgs.is_empty() {
            let preview: String = qt.chars().take(40).collect();
            self.checkpoint(format!("before: {preview}"));
        }
        // A WORK LIMIT, if the operator just asked for one.
        //
        // "keep working for the next hour" was unreadable to the model: no turn
        // carried a timestamp, so the phrase could only be pattern-matched and
        // "keep working" fifty minutes later carried no signal at all. Parsed
        // here, at the turn that says it, because the deadline runs from when
        // it was ASKED — "30 more minutes" an hour in means thirty from then,
        // not thirty from session start.
        //
        // Only ever SET, never cleared by a later message: an open-ended "keep
        // working" after an hour was granted is a continuation of that hour,
        // not a revocation of it. Clearing needs an explicit act, and there is
        // not one yet.
        if let Some(d) = logscope_fan::sessiontime::parse_work_limit(&qt) {
            self.work_limit = Some((wall_now(), d));
        }
        self.msgs.push(Msg::user(qt));
        let uid = self.msgs.len() - 1;
        self.msgs[uid].parent = parent;
        self.active_leaf = Some(uid); // path now ends at the new user turn for prompt-building
        self.hit_cap = false;
        let prompt = self.build_prompt(doc);
        self.last_prompt_tokens = est(&prompt); // for the live progress estimate
        // Stamp the model onto the turn it produced. convoprov marks only the CHANGES, so this is
        // what lets a conversation spanning four models read as a timeline instead of one model
        // having a bad day — which is how the template-mismatch replies were read for several turns.
        { let mut a = Msg::assistant(); a.model = self.stamp_model.clone(); self.msgs.push(a); }
        let aid = self.msgs.len() - 1;
        self.msgs[aid].parent = Some(uid);
        self.active_leaf = Some(aid);
        self.stream = Some(Stream { idx: aid, started: Instant::now(), tokens: 0, progress: None });
        self.last = None;
        // Baseline the counters as the generation is submitted, so the panel reports this generation
        // rather than everything since process start.
        self.metrics_base = crate::metricspanel::snapshot(0);
        self.metrics_start = Some(std::time::Instant::now());
        infer.generate(prompt);
    }

    /// Apply a per-message control: copy / edit / regenerate (reroll → kept as a branch variant) /
    /// delete / cycle variants. Structural ones rebuild the prompt from a checkpoint and re-stream.
    fn apply_msg_action(&mut self, ctx: &egui::Context, act: MsgAction, infer: &Inference, doc: &crate::analysis::Doc) {
        match act {
            MsgAction::Copy(i) => { if let Some(m) = self.msgs.get(i) { ctx.copy_text(m.text().to_string()); } }
            MsgAction::PrevVar(i) => { if let Some(m) = self.msgs.get_mut(i) { if m.cur > 0 { m.cur -= 1; } } }
            MsgAction::NextVar(i) => { if let Some(m) = self.msgs.get_mut(i) { if m.cur + 1 < m.variants.len() { m.cur += 1; } } }
            MsgAction::EditStart(i) => { if let Some(m) = self.msgs.get_mut(i) { m.edit_buf = m.text().to_string(); m.editing = true; } }
            MsgAction::EditCancel(i) => { if let Some(m) = self.msgs.get_mut(i) { m.editing = false; m.edit_buf.clear(); } }
            MsgAction::Delete(i) => { if self.stream.is_none() && i < self.msgs.len() { self.msgs.truncate(i); self.last = None; self.active_leaf = i.checked_sub(1); self.pruned.retain(|&p| p < self.msgs.len()); } }
            MsgAction::EditSave(i) => {
                if self.stream.is_some() || i >= self.msgs.len() { return; }
                let buf = std::mem::take(&mut self.msgs[i].edit_buf);
                self.msgs[i].editing = false;
                if buf.trim().is_empty() { return; }
                self.msgs[i].variants = vec![buf]; self.msgs[i].cur = 0;
                self.msgs.truncate(i + 1);                    // drop the now-stale reply + after
                let prompt = self.build_prompt_upto(i + 1, doc);
                // Stamp the model onto the turn it produced. convoprov marks only the CHANGES, so this is
        // what lets a conversation spanning four models read as a timeline instead of one model
        // having a bad day — which is how the template-mismatch replies were read for several turns.
        { let mut a = Msg::assistant(); a.model = self.stamp_model.clone(); self.msgs.push(a); }
                let idx = self.msgs.len() - 1;
                self.msgs[idx].parent = Some(i);
                self.active_leaf = Some(idx);
                self.pruned.retain(|&p| p < self.msgs.len());
                self.stream = Some(Stream { idx, started: Instant::now(), tokens: 0, progress: None });
                self.last = None;
                // Baseline the counters as the generation is submitted, so the panel reports this generation
        // rather than everything since process start.
        self.metrics_base = crate::metricspanel::snapshot(0);
        self.metrics_start = Some(std::time::Instant::now());
        infer.generate(prompt);
            }
            MsgAction::Regenerate(i) => {
                if self.stream.is_some() || i >= self.msgs.len() || self.msgs[i].role != Role::Assistant { return; }
                self.msgs.truncate(i + 1);                    // drop anything after this reply
                let prompt = self.build_prompt_upto(i, doc);  // history up to the preceding user turn
                self.msgs[i].variants.push(String::new());    // new branch alongside the old reply
                self.msgs[i].cur = self.msgs[i].variants.len() - 1;
                self.active_leaf = Some(i);
                self.stream = Some(Stream { idx: i, started: Instant::now(), tokens: 0, progress: None });
                self.last = None;
                // Baseline the counters as the generation is submitted, so the panel reports this generation
        // rather than everything since process start.
        self.metrics_base = crate::metricspanel::snapshot(0);
        self.metrics_start = Some(std::time::Instant::now());
        infer.generate(prompt);
            }
        }
    }

    /// Quick-reply chips shown above the composer: conversation starters when empty, follow-ups
    /// after a reply (flavored by whether the reply had code / a log is attached). Clicking fills
    /// the composer. (A true context-aware autocomplete inside the box is a separate, heavier
    /// piece — it needs a model draft per keystroke or ghost-text egui doesn't natively do.)
    fn suggestions(&self, doc: &crate::analysis::Doc) -> Vec<String> {
        if self.stream.is_some() { return Vec::new(); }
        if self.msgs.is_empty() {
            let mut v = vec!["Explain this code".to_string(), "Write a unit test".to_string()];
            if self.attach_log && !doc.lines.is_empty() { v.insert(0, "What's the fault in this log?".into()); }
            return v;
        }
        match self.msgs.last() {
            Some(m) if m.role == Role::Assistant && !m.text().is_empty() => {
                if m.text().contains("```") { vec!["Add error handling".into(), "Write tests for this".into(), "Explain the tricky part".into()] }
                else { vec!["Show me the code".into(), "Go deeper".into(), "Summarize".into()] }
            }
            _ => Vec::new(),
        }
    }

    /// Node-id completions for the composer. When you're mid-id-reference in a `:mergectx`/`:goto`
    /// command, the matching node ids, each paired with the input a click would produce (the partial
    /// token swapped for the full id). Empty unless the composer is actually mid-reference.
    fn id_completions(&self) -> Vec<(String, String)> {
        let s = &self.input;
        let t = s.trim_start();
        if !(t.starts_with(":mergectx") || t.starts_with(":mc") || t.starts_with(":goto")) { return Vec::new(); }
        let cut = s.rfind(|c: char| c == ',' || c == ' ' || c == ':').map(|i| i + 1).unwrap_or(0);
        let (head, tok) = s.split_at(cut);
        let tok = tok.trim();
        if tok.is_empty() || tok.starts_with('-') || tok.chars().all(|c| c.is_ascii_digit()) { return Vec::new(); }
        let pref = tok.to_ascii_lowercase();
        let mut out = Vec::new();
        for i in 0..self.msgs.len() {
            let id = crate::vimmode::short_id(i);
            if id.starts_with(&pref) { out.push((id.clone(), format!("{head}{id}"))); if out.len() >= 10 { break; } }
        }
        out
    }

    fn sync_worker(&mut self, infer: &Inference) {
        if self.sent_backend != Some(self.backend) { infer.set_backend(self.backend); self.sent_backend = Some(self.backend); }
        // Template: trust the worker's LIVE detected template over per-tab cached state. A tab created
        // after the model loaded (or whose Evt::Loaded another drain consumed) otherwise keeps a stale
        // default and silently builds Llama-3 prompts for a ChatML model — the markers byte-split
        // (prompt diag: "0 special tokens"), the turn structure never reaches the model, and output
        // degrades to word salad (seen on every Qwen3 while Llama models worked).
        // Feed the HUD from the engine's own counters while a reply is generating. These are
        // observations, not narration: each is a measured fact the engine reported, phrased minimally and
        // tagged with where it came from. The liaison model, when configured, rephrases them; until then
        // the raw line shows, which cannot be wrong.
        if self.stream.is_some() {
            let c = crate::metricspanel::snapshot(self.metrics_tokens.max(1))
                .since(&self.metrics_base);
            if c.moe_fills > 0 {
                self.hud.observe(format!("streaming experts — {} fills, {:.1} GiB moved",
                    c.moe_fills, c.moe_bytes as f64 / (1u64 << 30) as f64), "moe-stream");
            } else if c.submits > 0 {
                self.hud.observe(format!("{} GPU submits so far", c.submits), "engine");
            }
        } else if !self.hud.is_empty() {
            // a finished reply should not sit under a stale status line
            self.hud.clear();
            self.hud_narration.clear();
        }
        if let Some(t) = infer.detected_template() {
            if self.template != t { self.template = t; }
            // Also record it separately, so build_prompt can use the worker's value even if a generate is
            // submitted in the same frame as the load that changed it.
            self.live_template = Some(t);
        }
        let key = (self.ctx.n_ctx, self.ctx.max_tokens, (self.ctx.temperature * 100.0) as i32, self.ctx.history_turns);
        if self.sent_params != Some(key) { infer.set_params(self.params()); self.sent_params = Some(key); }
        // Drain a queued agent prompt (initial or next-turn) to the worker — this is where the non-blocking
        // agent actually submits generations, since `infer` is in scope here every frame.
        if let Some(prompt) = self.agent_pending_submit.take() {
            if let Some(g) = self.agent_grammar.clone().filter(|_| self.agent_run.is_some() || self.agent_fan.is_some()) {
                let mut p = self.params(); p.grammar = Some(g);
                infer.set_params(p);
                self.sent_params = None;   // forces plain params to be re-sent before the next chat turn
            }
            infer.generate(prompt);
        }
    }

    /// Finalize a non-blocking agent run: snapshot the workspace, surface the outcome, clear the driver.
    fn finish_agent_run(&mut self, stop: crate::workspace_exec::Stop) {
        self.agent_grammar = None;
        // The scoreboard, before the driver is consumed: one ledger line in the same terms as
        // `hive count` on a Claude Code transcript, so the two harnesses meet on one table.
        if let Some(run) = self.agent_run.as_ref() {
            let counts = run.counts(&stop);
            let _ = hive::ledger::append(hive::ledger::Entry::Note { text: format!("harness: {}", counts.render()) });
            self.harness_line = Some(counts.render());
        }
        let (files, plan, plan_summary, verified, turns, had_verifier, phase, patterns, compacted) = match self.agent_run.take() {
            Some(r) => (r.events().to_vec(), r.plan().steps.iter().map(|s| (s.text.clone(), s.done)).collect::<Vec<_>>(), r.plan().summary(), r.verified(), r.turns(), r.had_verifier(), r.phase(), r.pattern_report(), r.compacted_turns()),
            None => return,
        };
        self.agent_files = files;
        self.agent_plan = plan;
        let touched = self.agent_files.len();
        let stopdesc = match &stop {
            crate::workspace_exec::Stop::Done => "done".to_string(),
            crate::workspace_exec::Stop::StepLimit => format!("hit step limit ({turns} turns)"),
            // THE DRIVER KNOWS WHY IT STOPPED, SO SAY IT. Asked why it had
            // stopped, a small model answered "a self-imposed limit to
            // prevent unnecessary verbosity" — a confabulation, because
            // nothing about the halt was ever in its context. The reason
            // lives out here; the UI must not leave the user asking the model.
            crate::workspace_exec::Stop::Deadline =>
                format!("reached the time limit ({turns} turns) — raise the wall clock, or narrow the task"),
            // Two different halts share Stop::Stalled and deserve different words: the loop guard
            // caught REPETITION; the phase machine's Blocked is counted evidence that the error
            // count stopped falling — the run should change approach, not just retry.
            crate::workspace_exec::Stop::Stalled if phase == Some("blocked") =>
                format!("blocked — the error count stopped falling ({turns} turns; change approach)"),
            crate::workspace_exec::Stop::Stalled => format!("stopped — model was looping ({turns} turns)"),
            crate::workspace_exec::Stop::ModelError(e) => format!("model error: {e}"),
            crate::workspace_exec::Stop::LaneRefused { lane } => format!(
                "stopped — every write landed outside this worker's lane ({}); the task needs a \
different worker or a new one ({turns} turns)", lane.join(", ")),
        };
        // Durable per-repo learning: the pattern report ("survey + checked: reduced errors 3/4
        // times") lands as a shared doc, which `collect_shared_docs` injects into EVERY later run
        // over this workspace — observed fact from this repo, not advice. Written before the
        // snapshot so the snapshot carries it.
        if !patterns.is_empty() {
            if let Some(s) = self.gitmind_session.as_ref() {
                let dir = s.work_dir().join(".logscope").join("docs");
                let _ = std::fs::create_dir_all(&dir);
                let body = format!("# phase patterns — what moved the error count on this repo\n\n{}\n", patterns.join("\n"));
                let _ = std::fs::write(dir.join("phase-patterns.md"), body);
            }
        }
        let snap = self.gitmind_session.as_ref()
            .and_then(|s| crate::mount::snapshot(&s.ws, &format!("agent: {}", self.agent_task)).ok())
            .unwrap_or(false);
        // THREE STATES, NOT TWO. `verified` starts true when no verify command is configured, so a
        // run where nothing was checked rendered as "✓ verified" — which is worse than no badge,
        // because it is the reason someone stops looking.
        let vnote = crate::workspace_exec::VerifyBadge::of(verified, had_verifier).label();
        let snap_note = if snap { " · snapshotted" } else { "" };
        let phase_note = phase.map(|p| format!(" · phase: {p}")).unwrap_or_default();
        let cmp_note = match compacted { 0 => String::new(), n => format!(" · {n} turn(s) compacted") };
        // "plan 3/3" used to appear after three read-only turns, because the plan advanced per turn
        // rather than per write. The summary reports verified / pending / unverifiable apart, so a
        // step nobody can check never reads as a step that was done.
        let plan_note = if self.agent_plan.is_empty() { String::new() } else { format!(" · plan {plan_summary}") };
        let score = self.harness_line.as_ref().map(|l| format!(" · [{l}]")).unwrap_or_default();
        self.note = format!("agent: {stopdesc}{plan_note} · {touched} file(s){vnote}{phase_note}{cmp_note}{snap_note}{score} · :reapply to land");
    }

    /// Launch a fan of agent branches with `cfg`, seeding goals from `task`. Shared by :fan-agent and :docs.
    /// Turn-0 context for any agent/fan run over the loaded workspace: bounded file tree + shared
    /// crew docs. This is what makes "add more blocks" land on a model that can SEE the repo.
    /// Parse a patch-mode reply and apply it to the workspace.
    ///
    /// STRICT, and failing hunks are reported rather than forced. A hunk whose context does not match
    /// is a hunk aimed at a file the model was not looking at, and applying it by searching nearby
    /// would land a change on whatever happened to resemble it — valid-looking source that nobody
    /// asked for. `apply_file_patch_strict` already refuses; this surfaces the refusal.
    ///
    /// Nothing is written unless EVERY hunk in that file applied. A half-patched file is worse than an
    /// unpatched one: it compiles or it does not, and either way it no longer matches the diff the
    /// model produced or the source it was reasoning about.
    fn apply_patch_reply(&mut self, reply: &str) {
        let patches = crate::patchmode::parse_patches(reply);
        if patches.is_empty() {
            // NOT SILENT. Patch mode asks the model for unified diffs; a model
            // that answered in prose (or in a diff shape this parser does not
            // accept) produced a reply that changed no files and said nothing
            // about it — the mode reads as a black box with no response. Say
            // what happened, and say what the reply DID look like, since the
            // useful next move differs: re-ask, or turn the mode off.
            let shape = crate::patchmode::no_patch_reason(reply);
            self.note = format!("patch mode: no patches applied — {shape}. Nothing was written. Ask again, or turn patch mode off to get whole files.");
            return;
        }
        let Some(sess) = self.gitmind_session.as_ref() else {
            self.note = format!("patch mode: {} patch(es) parsed but no workspace is open to apply \
them to", patches.len());
            return;
        };
        let root = sess.work_dir().to_path_buf();
        let (mut applied, mut refused) = (0usize, Vec::new());
        for fp in &patches {
            let path = root.join(&fp.path);
            let Ok(orig) = std::fs::read_to_string(&path) else {
                refused.push(format!("{}: not readable under the workspace", fp.path));
                continue;
            };
            let out = crate::patchmode::apply_file_patch_strict(&orig, fp);
            if !out.is_clean() {
                // Report which hunks failed and why. A count alone tells nobody what to re-ask for.
                for line in out.report() { refused.push(format!("{}: {line}", fp.path)); }
                continue;
            }
            match std::fs::write(&path, &out.text) {
                Ok(()) => applied += 1,
                Err(e) => refused.push(format!("{}: write failed: {e}", fp.path)),
            }
        }
        self.note = if refused.is_empty() {
            format!("patch mode: {applied} file(s) patched")
        } else {
            for r in &refused { eprintln!("patch: {r}"); }
            format!("patch mode: {applied} applied, {} refused — see the log; nothing was force-fitted",
                    refused.len())
        };
    }

    /// The mounted workspace as chat-turn context: what is open, an
    /// orientation tree, and — the part that matters — a plain statement that
    /// the contents are READABLE and how to ask.
    ///
    /// Deliberately smaller than `agent_context`: an agent run is committed to
    /// working the repo, while a chat turn may be about anything, so this
    /// buys awareness rather than evidence. History is one line (the newest
    /// subject) for the same reason — enough to know it exists.
    /// The most descriptive files in the workspace, within `budget_bytes`.
    ///
    /// Walks the tree once, scores each path with
    /// `ctxplan::describes_project` (path shape only — no file is opened to
    /// decide whether to open it), then lets `ctxplan::plan` spend the budget.
    /// Only the files the plan actually selects are read.
    fn planned_files(&self, root: &std::path::Path, budget_bytes: usize) -> String {
        // Serve the memo when the inputs have not changed. See `planned_cache`:
        // this is on a per-frame path, and walking a workspace tree plus reading
        // the selected files at 60 Hz is what produced the endlessly repeating
        // "workspace files: …" log line.
        // Keyed on the QUESTION as well, now that selection depends on it —
        // otherwise turn one's plan is served for the rest of the conversation
        // and the query-awareness never takes effect after the first message.
        let q = self.msgs.iter().rev().find(|m| m.role == Role::User)
            .map(|m| m.text().to_string()).unwrap_or_default();
        if let Some((r, b, s)) = self.planned_cache.borrow().as_ref() {
            if r == root && *b == budget_bytes && self.planned_q.borrow().as_deref() == Some(q.as_str()) {
                return s.clone();
            }
        }
        let out = self.plan_files_uncached(root, budget_bytes);
        *self.planned_cache.borrow_mut() = Some((root.to_path_buf(), budget_bytes, out.clone()));
        *self.planned_q.borrow_mut() = Some(q);
        out
    }

    fn plan_files_uncached(&self, root: &std::path::Path, budget_bytes: usize) -> String {
        // The most recent user turn is the question the selection should serve.
        // Empty (no turns yet) falls back to path-shape ranking, which is the
        // right answer for an opening "what is this".
        let question = self.msgs.iter().rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.text().to_string())
            .unwrap_or_default();
        use crate::ctxplan::{plan, relevance_for_query, Budget, FileEntry};
        const SKIP_DIR: [&str; 7] =
            ["target", ".git", ".gitmind", ".logscope", "node_modules", "dist", ".cargo"];
        // Bounded walk: a workspace can be enormous and this runs while building
        // a prompt. The cap is on entries CONSIDERED, not selected.
        let mut files: Vec<FileEntry> = Vec::new();
        let mut queue = std::collections::VecDeque::from([(root.to_path_buf(), 0usize)]);
        while let Some((dir, depth)) = queue.pop_front() {
            if depth > 6 || files.len() > 4000 { break; }
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') { continue; }
                if p.is_dir() {
                    if !SKIP_DIR.contains(&name.as_str()) { queue.push_back((p, depth + 1)); }
                    continue;
                }
                let Ok(rel) = p.strip_prefix(root) else { continue };
                let rel = rel.to_string_lossy().replace('\\', "/");
                // Ranked against the LIVE QUESTION, not just path shape. See
                // ctxplan::relevance_for_query: "add a new theme bright pink"
                // surfaced Cargo.toml and buried palette.rs, and the model
                // invented a `themes` array into the manifest it was shown.
                let r = relevance_for_query(&rel, &question);
                if r <= 0.0 { continue; }
                let bytes = e.metadata().map(|m| m.len() as usize).unwrap_or(0);
                files.push(FileEntry { path: rel, bytes, relevance: r, sections: Vec::new() });
            }
        }
        if files.is_empty() { return String::new(); }
        // `Budget::for_context` rather than `Budget { max_tokens, ..default() }`.
        // The default keeps `whole_file_max_tokens` at 1500 no matter how small
        // the budget is, so ONE medium file can eat the lot. Caught by running
        // this over apps/switcheroo: a 1405-token HOMEKIT.md took 70% of a
        // 2000-token budget and crowded out every source file, which is the
        // opposite of what a summary wants. `for_context` caps any single whole
        // file at a quarter of the budget, so the manifests and entry points
        // all fit. Reserves are 0 here because `budget_bytes` is already the
        // block's share of the prompt, not the model's whole window.
        let b = Budget::for_context(budget_bytes / 4, 0, 0);
        let p = plan(&files, &b);
        // Read only what was selected — the savings are the files never opened.
        let body = p.assemble(|path, ranges| {
            let full = std::fs::read_to_string(root.join(path)).unwrap_or_default();
            if ranges.is_empty() { return crate::read_harness::sanitize(&full); }
            let mut s = String::new();
            for (a, z) in ranges {
                let (a, z) = ((*a).min(full.len()), (*z).min(full.len()));
                if a < z { s.push_str(&full[a..z]); s.push('\n'); }
            }
            crate::read_harness::sanitize(&s)
        });
        // Keep the per-file verdicts, not just the counts. The counts say a
        // selection happened; the verdicts are what let someone see that
        // palette.rs was ranked 0.17 and dropped while two manifests at 0.90
        // went in — which is the field case this planner exists to fix.
        *self.planned_why.borrow_mut() = p.explain(&files);
        crate::diag!("workspace files: {} considered, {} whole + {} peeked, {} skipped, \
~{} tokens of file content in the prompt",
            files.len(), p.looped_in(), p.peeked(), p.skipped(), p.used_tokens);
        body
    }

    fn workspace_block(&self) -> Option<String> {
        let sess = self.gitmind_session.as_ref()?;
        let root = sess.work_dir();
        let class = self.read_class();
        let b = crate::read_harness::budget_for_window(class, self.ctx.n_ctx);
        let tree_cap = match class {
            crate::read_harness::ModelClass::Small => 40,
            crate::read_harness::ModelClass::Mid => 120,
            crate::read_harness::ModelClass::Large => 400,
            crate::read_harness::ModelClass::Sota => 1200,
        };
        let tree = crate::workspace_exec::repo_tree(root, tree_cap);
        // THE INSTRUCTION USED TO BE A LIE, and it is the likeliest reason a
        // small model looks incapable here.
        //
        // This said "Its files are readable — read the file instead of reasoning
        // from the error text alone, and say which file you read." A plain chat
        // turn cannot read anything: the read loop lives in the AGENT path
        // (`AgentRun`/`AgentStep`), and nothing in the chat reply path parses a
        // read request or re-submits. So the model was told to do something
        // impossible, given 40 file NAMES and one commit subject, and then asked
        // to "summarize the codebase". No model summarises a repository from a
        // truncated directory listing; a fluent one produces confident vagueness,
        // which reads as the model being weak rather than the prompt being empty.
        //
        // Two changes. The instruction now describes what is actually available,
        // and the shared docs go in — the same `collect_shared_docs` the agent
        // path already uses. A README is what a person reads first when asked to
        // summarise a repository, and it is the difference between names and
        // content. Budgeted by the same class, so a 1B still gets a short block.
        let mut out = format!(
            "A workspace is OPEN at {}. What follows is everything you have been given about \
it — a partial file tree and any shared notes. You CANNOT open files in this conversation. \
If answering needs a file you have not been shown, say which file and why rather than \
guessing at its contents.\n\n",
            root.display());
        out.push_str(&crate::read_harness::fence("repository tree", &root.display().to_string(), &tree, &b, None));
        // Content, not just names. Without this the block is a listing, and a
        // question about what the code DOES has nothing to stand on.
        let docs = crate::workspace_exec::collect_shared_docs(root, b.block_bytes);
        if !docs.is_empty() {
            out.push_str(&crate::read_harness::fence("shared notes", ".logscope/docs", &docs, &b, None));
        }
        // ACTUAL FILES, chosen by the planner that had no callers.
        //
        // ctxplan.rs computes exactly this — which files fit a token budget,
        // whole vs peeked vs skipped — and sat unused for its whole life because
        // nothing produced the `relevance` it needs. `ctxplan::describes_project`
        // is that missing half. So the summary block is now manifests and entry
        // points chosen deliberately, rather than whatever the tree walk reached
        // first.
        //
        // Budgeted from `block_bytes` (the same class budget the rest of this
        // function uses) rather than the model's full window, so this stays a
        // bounded addition to a system prompt rather than becoming the prompt.
        let planned = self.planned_files(root, b.block_bytes);
        if !planned.is_empty() {
            out.push_str(&crate::read_harness::fence(
                "key files", "chosen by relevance under the context budget", &planned, &b, None));
        }
        if let Some(c) = crate::read_harness::read_git_log(root, 1).first() {
            out.push_str(&crate::read_harness::fence(
                "most recent commit", "git log -n 1",
                &format!("{} {}", c.short, c.subject), &b, None));
        }
        // THE MISSING DIAGNOSTIC. "the model is bad" and "the prompt was a
        // directory listing" were indistinguishable from outside; one line
        // separates them. Reported once per distinct shape rather than per turn,
        // so it informs without becoming noise in a long chat.
        {
            use std::sync::atomic::{AtomicU64, Ordering};
            static LAST: AtomicU64 = AtomicU64::new(u64::MAX);
            let shape = (tree.lines().count() as u64) << 32 | out.len() as u64 & 0xffff_ffff;
            if LAST.swap(shape, Ordering::Relaxed) != shape {
                crate::diag!("workspace context: {} tree entries (cap {tree_cap}, class {:?}), \
{} bytes of shared notes, {} bytes total in the system prompt. This is ALL the model can see of \
the repository — a chat turn cannot open files.",
                    tree.lines().count(), class, docs.len(), out.len());
            }
        }
        Some(out)
    }

    /// The model's capability class, from the engine's own label. Everything
    /// budgeted — file bytes, history depth, whether bodies appear — keys on
    /// this rather than on constants that suited whichever model was loaded
    /// the day the constant was written.
    fn read_class(&self) -> crate::read_harness::ModelClass {
        crate::read_harness::ModelClass::resolve(None, &self.model_label())
    }

    /// The workspace as EVIDENCE: tree, shared docs and recent history, each
    /// sanitized, budgeted to the model's class and fenced with its
    /// provenance.
    ///
    /// Before this the caps were fixed (300 tree entries, 6000 doc bytes, no
    /// history at all) — a 1B drowned in the same context a 70B found thin,
    /// and neither could see what the repository had recently DONE, which is
    /// most of what "work on this repo" means. Nothing here is fenced twice:
    /// repo_tree and collect_shared_docs still produce the text; the harness
    /// decides how much and wraps it.
    fn agent_context(&self) -> String {
        let Some(sess) = self.gitmind_session.as_ref() else { return String::new() };
        let root = sess.work_dir();
        let class = self.read_class();
        let b = crate::read_harness::budget_for_window(class, self.ctx.n_ctx);

        // Tree entries scale with what the class can hold; the byte cap inside
        // fence() is the real backstop.
        let tree_cap = match class {
            crate::read_harness::ModelClass::Small => 80,
            crate::read_harness::ModelClass::Mid => 300,
            crate::read_harness::ModelClass::Large => 900,
            crate::read_harness::ModelClass::Sota => 3000,
        };
        let tree = crate::workspace_exec::repo_tree(root, tree_cap);
        let mut out = crate::read_harness::fence("repository tree", &root.display().to_string(), &tree, &b, None);

        let docs = crate::workspace_exec::collect_shared_docs(root, b.block_bytes);
        if !docs.is_empty() {
            out.push_str(&crate::read_harness::fence("shared notes", ".logscope/docs", &docs, &b, None));
        }

        // Recent history, if this is a git worktree. Hostile refs and messages
        // are contained by git_history_block; a repo without git simply
        // contributes nothing.
        let commits = crate::read_harness::read_git_log(root, b.git_entries);
        if !commits.is_empty() {
            out.push_str(&crate::read_harness::git_history_block(
                &commits, &b, &format!("{} @ HEAD", root.display())));
        }
        out
    }

    /// Start a single agent run over the loaded workspace (shared by `:agent` and the plain-message
    /// auto-route when a workspace is loaded). Assumes idle-ness was checked by the caller's flow;
    /// re-checks defensively and notes instead of colliding.
    /// Write the persisted choices back.
    ///
    /// Called from the pickers rather than every frame: this touches the filesystem, and a UI that
    /// writes a config file at 60 Hz is a UI that will be blamed for disk wear. The space is read
    /// from the live theme rather than stored twice, so the file cannot disagree with the screen.
    fn persist_prefs(&self) {
        let mut p = crate::prefs::load();   // keep unknown keys another build may own
        p.nav_layout = self.nav_layout;
        p.profiles = self.profiles_manual.clone();
        p.space = Some(theme::active().name.to_string());
        crate::prefs::save(&p);
    }

    /// Staff `task` against the seed roster's lane rule and, on a match, confine `cfg` to that
    /// worker's `owns` — the cheap-rule half of the Delegator (`docs/12-queen-and-workers.md`),
    /// shared by every entry point that starts an agent run so `:agent`, `:fan-agent` and `:docs`
    /// cannot drift into three different staffing policies. Returns the HUD line to post, if any.
    fn stage_lane(&self, cfg: &mut crate::workspace_exec::AgentConfig, task: &str) -> Option<String> {
        match self.roster.staff(task) {
            logscope_fan::roster::Staffing::Reuse { worker, why } => {
                if let Some(spec) = self.roster.get(&worker) {
                    // Lane AND whitelist, from one value, or neither. This used to be
                    // `cfg.owns = spec.owns.clone()` alone: the spec's `tools` never reached the
                    // run, and an empty `owns` means read-only THERE and unrestricted HERE.
                    let c = logscope_fan::deploy::Confinement::of(spec);
                    cfg.owns = c.runtime_owns();
                    cfg.tools = c.tools.clone();
                    return Some(format!("staffed to {} ({why})", c.describe()));
                }
                None
            }
            // Named rather than silently falling through to an unrestricted run — an operator
            // running the SAME kind of task repeatedly should notice the roster never grows and
            // reach for `:queen` instead of every run staying a general, unconfined agent.
            logscope_fan::roster::Staffing::Ambiguous { candidates } =>
                Some(format!("ambiguous between {} — running unconfined; :queen this task \
to have the Queen split it, or name the worker directly", candidates.join(", "))),
            logscope_fan::roster::Staffing::Create => None,
        }
    }

    /// `:enlist` — find the latest assistant reply along the active lineage, parse it as a
    /// `WorkerSpec` (the Queen's output), and add it to the roster if it validates. Split out of
    /// `run_command` (which needs an `egui::Ui` this crate has no test harness for constructing) so
    /// the actual chain-walk + parse + validate + add pipeline is unit-testable end to end, not just
    /// its pieces individually.
    fn do_enlist(&mut self) -> String {
        let mut cur = self.active_leaf;
        let mut reply: Option<String> = None;
        while let Some(i) = cur {
            let Some(m) = self.msgs.get(i) else { break };
            if m.role == Role::Assistant && !m.text().trim().is_empty() {
                reply = Some(m.text().to_string());
                break;
            }
            cur = m.parent;
        }
        let Some(text) = reply else {
            return ":enlist — no assistant reply to parse yet; :queen a task and send it first".into();
        };
        // A HERO is a spec plus a machine. If the reply has a `State NAME:` line it is one — parse
        // and validate it as such (against a stable holding the seeds and the heroes already
        // enlisted, so sub-hero references and duplicate names are checked for real), and put its
        // spec on the roster too: a hero is a worker, and the Delegator should be able to see it.
        if text.lines().any(|l| l.trim_start().starts_with("State ")) {
            return match logscope_fan::hero::Hero::parse(&text) {
                Err(e) => format!(":enlist — the hero didn't parse: {e}"),
                Ok(hero) => {
                    let mut st = logscope_fan::hero::seed_stable(crate::workspace_exec::RUNTIME_TOOLS);
                    for h in &self.heroes { let _ = st.add(h.clone(), crate::workspace_exec::RUNTIME_TOOLS); }
                    match st.add(hero.clone(), crate::workspace_exec::RUNTIME_TOOLS) {
                        Err(e) => format!(":enlist — hero {} refused: {e}", hero.spec.name),
                        Ok(()) => {
                            let (name, n_states) = (hero.spec.name.clone(), hero.machine.states.len());
                            let gate = match &hero.gate { Some(g) => format!("fires {:?}", g.on), None => "called only".into() };
                            self.roster.add(hero.spec.clone());
                            self.heroes.push(hero);
                            format!("🦸 enlisted hero {name} — {n_states} state(s), {gate}; {} hero(es) this session", self.heroes.len())
                        }
                    }
                }
            };
        }
        match logscope_fan::queen::WorkerSpec::parse(&text) {
            Err(e) => format!(":enlist — the reply didn't parse as a spec: {}", e.correction()),
            Ok(spec) => match spec.validate(crate::workspace_exec::RUNTIME_TOOLS) {
                Err(e) => format!(":enlist — {} refused: {}", spec.name, e.correction()),
                Ok(()) => {
                    let name = spec.name.clone();
                    self.roster.add(spec);
                    format!("👑 enlisted {name} — {} worker(s) on the roster now", self.roster.len())
                }
            },
        }
    }

    fn start_agent_task(&mut self, task: String, via: &str) {
        if task.is_empty() { self.note = format!("{via} needs a task"); return; }
        if self.gitmind_session.is_none() {
            self.note = format!("{via} — load a workspace first (:load-gitmind <path>)"); return;
        }
        if self.agent_run.is_some() { self.note = format!("{via} — a run is already active (:stop to cancel)"); return; }
        if self.stream.is_some() { self.note = format!("{via} — the model is busy; wait or :stop"); return; }
        let work_dir = self.gitmind_session.as_ref().unwrap().work_dir().to_path_buf();
        let mut cfg = crate::workspace_exec::AgentConfig::default();
        cfg.context = self.agent_context();
        if let Some(line) = self.stage_lane(&mut cfg, &task) {
            self.hud.observe(line, "delegator");
        }
        // Size the run's budgets to the model actually loaded. Without this the workspace resolves
        // to Mid for everything — the harness's "unknown" — so a 1B drowns in reads it cannot hold
        // and a large model gets a transcript bound far below what its window could carry.
        cfg.model_label = self.model_label();
        // Hero reflexes (logscope_fan::hero): model-free procedures that read the code at a compiler
        // error / find a failed test BEFORE the model has to ask — one generation saved per firing.
        // On in the app; LOGSCOPE_REFLEXES=0 is the off switch while this is new.
        cfg.reflexes = std::env::var("LOGSCOPE_REFLEXES").as_deref() != Ok("0");
        cfg.extra_heroes = self.heroes.clone();
        // The mode picker records the intent; this is where the intent takes effect. StatefulGuide
        // arms engmode's phase machine (evidence-decided position, claim-gating, stall→blocked) and
        // green-state checkpoints — the long-horizon posture for a big `.gitmind` tree.
        if self.coord_mode == crate::coordinator::Mode::StatefulGuide {
            cfg.stateful_guide = true;
            cfg.checkpoint_on_green = true;
            // PROOF FOLLOWS FROM THE MODE'S OWN PREMISE. StatefulGuide exists because a model
            // saying TASK_COMPLETE is not evidence; a green suite is the same claim one level down,
            // since it cannot tell a change that fixed something from a change that did nothing.
            // The mode that refuses the first should refuse the second. Costs one extra suite run,
            // once, at the claim — which for an unattended long-horizon run is nothing against
            // landing work that changed no observable behaviour.
            cfg.require_proof = true;
        }
        // AUTOPILOT: the working discipline follows the task. A shader bug gets the GPU profile's
        // rule for verification (diff against a reference) rather than the generic one; anything
        // unrecognised gets the base posture, which is the measured default for this operator.
        // A manual selection, when one has been made, wins over the suggestion.
        cfg.profiles = if self.profiles_manual.is_empty() {
            crate::engprofile::suggest(&task)
        } else { self.profiles_manual.clone() };
        let profile_names: Vec<&str> = cfg.profiles.iter().map(|p| p.profile().name).collect();
        self.agent_grammar = tool_grammar_for(&cfg.tools, &cfg.gate);
        let (run, first) = crate::workspace_exec::AgentRun::start(work_dir, &task, cfg);
        let guided = run.phase().is_some();
        self.agent_run = Some(run);
        self.agent_task = task.clone();
        self.agent_pending_submit = Some(first);
        self.agent_files.clear();
        self.agent_plan.clear();
        // Name the posture in the status line. An autopilot that silently changes how the model
        // works is one the operator cannot correct — the whole reason the mode picker states what
        // each mode actually does.
        let posture = profile_names.join(" + ");
        self.note = if guided {
            format!("agent started (stateful guide, {posture}): {task} — phase: survey")
        } else {
            format!("agent started ({posture}): {task} — surveying the workspace…")
        };
    }

    fn start_fan(&mut self, task: &str, cfg: crate::workspace_exec::AgentConfig, doc_fan: bool, label: &str) {
        let mut cfg = cfg;
        if cfg.context.is_empty() { cfg.context = self.agent_context(); } // every branch starts sighted
        // …and every branch sized to the model actually loaded. `start_agent_task` got this and the
        // fan did not, so fan branches were still resolving to the harness's "unknown" Mid class for
        // their read chunks, git entries and transcript bound.
        if cfg.model_label.is_empty() { cfg.model_label = self.model_label(); }
        let task = task.trim().to_string();
        if task.is_empty() {
            self.note = format!(":{label} needs a task");
        } else if self.gitmind_session.is_none() {
            self.note = format!(":{label} — load a workspace first (:load-gitmind <path>)");
        } else if self.agent_run.is_some() || self.agent_fan.is_some() || self.stream.is_some() {
            self.note = format!(":{label} — the model is busy (:stop to cancel)");
        } else {
            let base_goal = self.msgs.iter().rev().find(|m| m.role == Role::User).map(|m| m.text().to_string()).unwrap_or_else(|| task.clone());

            // BRANCHES FROM THE PLANNER, when there is a plan to work from.
            //
            // `coordinator::plan()` has never been consumed, and the reason was one missing input:
            // it fans out over `Task.subtasks` and nothing produced subtasks, so it always degraded
            // to the single worker every caller already had. The plan the model wrote — parsed on
            // turn one, its steps carrying the files they name — is that input. Using it means the
            // same plan the operator read both fans the work out and decides afterwards whether it
            // was done.
            //
            // Falls back to the reaction seeder when there is no plan yet: a fan asked for before
            // any run has planned anything still has to do something sensible.
            let plan_steps: Vec<String> = self.agent_plan.iter().map(|(t, _)| t.clone())
                .filter(|t| !t.trim().is_empty()).collect();
            let planned = if plan_steps.len() >= 2 {
                let t = crate::planrun::task_from_steps(&base_goal, &plan_steps, &self.agent_context());
                // A budget on total RUNS, not units: aggressive costs three runs per unit, and the
                // count below is the branch list itself, so it cannot describe work that will not
                // happen (an earlier version reported `units * best_of` while running `units`).
                let (bs, dropped) = crate::planrun::branches(self.coord_mode, &t, 6);
                if dropped > 0 {
                    self.note = format!("{label}: {} run(s) from {} plan step(s), {dropped} step(s) over the budget",
                        bs.len(), plan_steps.len());
                }
                bs
            } else { Vec::new() };

            let goals: Vec<(String, String, String)> = if !planned.is_empty() {
                planned.into_iter().map(|b| (b.goal, b.label, b.system)).collect()
            } else {
                let seeded = crate::workspace_exec::seed_agent_branches(&task, &base_goal, 4);
                let seeded = if seeded.is_empty() { vec![task.clone()] } else { seeded };
                seeded.into_iter().enumerate()
                    .map(|(i, g)| (g, format!("branch {}", i + 1), String::new())).collect()
            };
            let session = self.gitmind_session.as_ref().unwrap();
            let mount = match crate::mount::detect(&session.project) { Ok(m) => m, Err(e) => { self.note = format!(":{label} — {e}"); return; } };
            let fan_root = std::env::temp_dir().join("logscope-fan");
            let mut specs = Vec::new();
            for (i, (goal, blabel, persona)) in goals.iter().enumerate() {
                match crate::mount::isolate(&mount, &fan_root.join(format!("branch{i}"))) {
                    Ok(ws) => specs.push((ws.dir.clone(), goal.clone(), blabel.clone(), persona.clone())),
                    Err(e) => { self.note = format!(":{label} — isolate failed: {e}"); return; }
                }
            }
            let diff_fn = Box::new(|dir: &std::path::Path| -> String {
                std::process::Command::new("git").arg("-C").arg(dir).args(["diff", "logscope-base"])
                    .output().ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).chars().take(8000).collect())
                    .unwrap_or_default()
            });
            let mut fan = crate::workspace_exec::AgentFan::planned(specs, &cfg, diff_fn);
            self.agent_candidates.clear();
            self.agent_task = task.clone();
            self.fan_is_docs = doc_fan;
            if self.lanes.is_some() {
                // Lane mode: every branch decodes CONCURRENTLY on the pool (one resident weights copy,
                // one KV cache per lane) — the single Inference worker stays free for the chat itself.
                // Jobs queue safely even if the pool's model is still loading; lanes pull on Ready.
                let lane_n = self.lanes.as_ref().map(|p| p.lanes()).unwrap_or(1);
                fan.set_width(lane_n);
                let steps = fan.fill();
                let n = fan.len();
                let params = logscope::llm_backend::GenParams {
                    max_tokens: self.ctx.max_tokens as usize, n_ctx: self.ctx.n_ctx, n_batch: self.ctx.n_ctx,
                    temperature: self.ctx.temperature, ..Default::default()
                };
                let mut launched = 0;
                if let Some(pool) = self.lanes.as_mut() {
                    for s in steps {
                        if let crate::workspace_exec::FanStep::Submit { branch, prompt } = s {
                            let id = pool.submit(prompt, params.clone());
                            self.lane_jobs.insert(id, branch);
                            launched += 1;
                        }
                    }
                }
                self.agent_grammar = tool_grammar_for(&[], &cfg.gate);
                self.agent_grammar = tool_grammar_for(&[], &cfg.gate);
        self.agent_fan = Some(fan);
                let ready = if self.lanes_ready { "decoding in parallel" } else { "queued (lanes still loading)" };
                self.note = format!("{label}: {n} branches — {launched} {ready} on {lane_n} lane(s)");
            } else {
                match fan.next() {
                    crate::workspace_exec::FanStep::Submit { prompt, .. } => {
                        self.agent_pending_submit = Some(prompt);
                        let n = fan.len();
                        self.agent_grammar = tool_grammar_for(&[], &cfg.gate);
                self.agent_fan = Some(fan);
                        self.note = format!("{label}: {n} branches spinning up — planning… (tip: :lanes 2 runs them in parallel)");
                    }
                    crate::workspace_exec::FanStep::AllDone => self.note = format!(":{label} — nothing to run"),
                }
            }
        }
    }

    /// Merge the doc fan's branches into one organized tree: union each branch's `docs/**`, regenerate the
    /// index from front-matter, and commit into the project. Collisions (a path written by >1 branch) are
    /// reported for the user to resolve rather than silently overwritten.
    fn merge_fan_docs(&mut self) {
        if self.agent_candidates.is_empty() {
            self.note = ":merge-docs — no fan branches (run :docs <task> first)".into();
            return;
        }
        let Some(session) = self.gitmind_session.as_ref() else {
            self.note = ":merge-docs — no .gitmind session loaded".into(); return;
        };
        let project = session.project.clone();
        let title = session.name.clone();
        let branches: Vec<(String, std::path::PathBuf)> = self.agent_candidates.iter()
            .map(|c| (c.label.clone(), c.root.clone())).collect();
        let (mut result, _) = crate::workspace_exec::merge_branch_docs(&branches, &title);
        if result.merged.is_empty() {
            self.note = ":merge-docs — the branches produced no docs (docs/**/*.md)".into();
            return;
        }
        // Reconcile genuine conflicts with the depth model if one is loaded (near-dups already auto-resolved).
        let mut reconciled = 0;
        if !result.needs_merge.is_empty() {
            let params = logscope::llm_backend::GenParams {
                max_tokens: self.ctx.max_tokens as usize, n_ctx: self.ctx.n_ctx, n_batch: self.ctx.n_ctx,
                temperature: self.ctx.temperature, ..Default::default()
            };
            if let Some(mut llm) = crate::workspace_exec::LocalLlm::from_active(params) {
                reconciled = crate::workspace_exec::reconcile_doc_conflicts(&mut llm, &mut result);
            }
        }
        // regenerate the index AFTER reconciliation (reconciled bodies may change titles/summaries)
        let scanned: Vec<(String, String)> = result.merged.clone();
        let index = crate::docorg::build_index_for(&scanned, "");
        let index_md = crate::docorg::render_index_markdown(&index, &title);
        let mut files = result.merged.clone();
        files.push(("docs/README.md".to_string(), index_md));
        match crate::gitmind_session::write_and_commit(&project, &files, "merge docs") {
            Ok(hash) => {
                let auto = if result.auto_resolved.is_empty() { String::new() } else { format!(" · {} auto-merged", result.auto_resolved.len()) };
                let rec = if reconciled > 0 { format!(" · {reconciled} reconciled") } else { String::new() };
                let coll = if result.collisions.is_empty() { String::new() }
                    else { format!(" · ⚠ {} unresolved conflict(s): {}", result.collisions.len(),
                        result.collisions.iter().map(|c| c.path.clone()).collect::<Vec<_>>().join(", ")) };
                self.agent_candidates.clear();
                self.note = format!("merged {} docs → commit {} · index regenerated{auto}{rec}{coll}", result.merged.len(), &hash[..hash.len().min(10)]);
            }
            Err(e) => self.note = format!(":merge-docs — {e}"),
        }
    }

    /// Land fan branch `n` (1-based) over the project and clear the candidates. Shared by :pick and the
    /// in-scene candidate panel click.
    fn pick_branch(&mut self, n: usize) {
        if self.agent_candidates.is_empty() {
            self.note = ":pick — no fan candidates (run :fan-agent first)".into();
        } else if n == 0 || n > self.agent_candidates.len() {
            self.note = format!(":pick <n> — choose 1..{}", self.agent_candidates.len());
        } else if let Some(session) = self.gitmind_session.as_ref() {
            let cand = self.agent_candidates[n - 1].clone();
            let project = session.project.clone();
            match crate::gitmind_session::reapply_dir(&cand.root, &project, &format!("fan {}", cand.label)) {
                Ok(hash) => {
                    self.agent_candidates.clear();
                    self.note = format!("picked {} → commit {} · {} file(s) landed", cand.label, &hash[..hash.len().min(10)], cand.files);
                }
                Err(e) => self.note = format!(":pick — {e}"),
            }
        } else {
            self.note = ":pick — no .gitmind session loaded".into();
        }
    }

    /// A live reaction during an active RUNNING fan seeds more parallel branches (5): each isolated in its
    /// own workspace, joining the round-robin without stopping the in-flight work. Returns true if it seeded.
    /// If instead a SINGLE run is active, the reaction FORKS it into a fan (the run becomes branch 1,
    /// preserving its work; the reaction's approaches become branches 2+).
    fn seed_reaction_branches(&mut self, reaction: &str) -> bool {
        // Case A: a single run is active → upgrade it into a fan around the reaction.
        if self.agent_run.is_some() && self.agent_fan.is_none() {
            return self.fork_run_to_fan(reaction);
        }
        let running = self.agent_fan.as_ref().map_or(false, |f| !f.reviewing() && !f.all_done());
        if !running { return false; }
        let project = match self.gitmind_session.as_ref() { Some(s) => s.project.clone(), None => return false };
        let base_goal = self.agent_task.clone();
        let goals = crate::workspace_exec::seed_agent_branches(reaction, &base_goal, 3);
        if goals.is_empty() { return false; }
        let mount = match crate::mount::detect(&project) { Ok(m) => m, Err(_) => return false };
        let fan_root = std::env::temp_dir().join("logscope-fan");
        let start_idx = self.agent_fan.as_ref().map(|f| f.len()).unwrap_or(0);
        let mut added = 0usize;
        for (k, goal) in goals.into_iter().enumerate() {
            if let Ok(ws) = crate::mount::isolate(&mount, &fan_root.join(format!("react{}", start_idx + k))) {
                if let Some(f) = self.agent_fan.as_mut() {
                    if f.add_branch(ws.dir.clone(), &goal, &format!("reaction {}", start_idx + k + 1)) { added += 1; }
                }
            }
        }
        if added > 0 { self.note = format!("reaction seeded {added} branch(es) — spinning up in parallel"); true } else { false }
    }

    /// Fork the active single `agent_run` into a fan on a reaction: the run continues as branch 1 (its work
    /// preserved), and the reaction's approaches spin up as branches 2+ — so ANY run becomes parallel the
    /// moment you react, without stopping or discarding the in-flight work.
    fn fork_run_to_fan(&mut self, reaction: &str) -> bool {
        let project = match self.gitmind_session.as_ref() { Some(s) => s.project.clone(), None => return false };
        let base_goal = self.agent_task.clone();
        let goals = crate::workspace_exec::seed_agent_branches(reaction, &base_goal, 3);
        if goals.is_empty() { return false; }
        let run = match self.agent_run.take() { Some(r) => r, None => return false };
        // the run's next prompt is queued in agent_pending_submit if it's between turns; None ⇒ mid-generation
        let pending = self.agent_pending_submit.take();
        let cfg = crate::workspace_exec::AgentConfig::default();
        let diff_fn = Box::new(|dir: &std::path::Path| -> String {
            std::process::Command::new("git").arg("-C").arg(dir).args(["diff", "logscope-base"])
                .output().ok().map(|o| String::from_utf8_lossy(&o.stdout).chars().take(8000).collect()).unwrap_or_default()
        });
        let mut fan = crate::workspace_exec::AgentFan::adopt(run, pending, &base_goal, "original", &cfg, diff_fn);
        // add the reaction's parallel approaches in fresh isolated workspaces
        let mut added = 0usize;
        if let Ok(mount) = crate::mount::detect(&project) {
            let fan_root = std::env::temp_dir().join("logscope-fan");
            for (k, goal) in goals.into_iter().enumerate() {
                if let Ok(ws) = crate::mount::isolate(&mount, &fan_root.join(format!("fork{k}"))) {
                    if fan.add_branch(ws.dir.clone(), &goal, &format!("reaction {}", k + 1)) { added += 1; }
                }
            }
        }
        // if the adopted branch was between turns, re-arm its queued prompt through the fan
        if self.agent_pending_submit.is_none() {
            if let crate::workspace_exec::FanStep::Submit { prompt, .. } = fan.next() {
                self.agent_pending_submit = Some(prompt);
            }
        }
        let n = fan.len();
        self.fan_is_docs = false;
        self.agent_fan = Some(fan);
        self.note = format!("forked into a fan: original + {added} reaction branch(es) = {n} parallel");
        true
    }

    /// Finalize a fan: fold the per-branch summaries for pruning. Doesn't auto-land anything — the user
    /// picks the keeper (:pick N) after comparing. Highlights which branches verified.
    /// Drain the lane pool's tagged events into the fan and keep every free lane loaded. Called once per
    /// frame — the pool is a second, PARALLEL event source next to the single `Inference` worker (whose
    /// fan intercept lives in the Evt loop). Two-phase for borrow clarity: collect events, apply them to
    /// the fan (collecting backfill submissions), then re-borrow the pool to submit those.
    fn pump_lanes(&mut self) {
        if self.lanes.is_none() { return; }
        let mut evts = Vec::new();
        if let Some(pool) = self.lanes.as_mut() {
            while let Ok(e) = pool.rx.try_recv() { evts.push(e); }
        }
        if evts.is_empty() && self.lane_jobs.is_empty() { return; }
        let mut steps: Vec<crate::workspace_exec::FanStep> = Vec::new();
        let mut dead = false;
        for e in evts {
            match e {
                crate::lanes::LaneEvt::Ready { lanes, model, instance_bytes, .. } => {
                    self.lanes_ready = true;
                    // The one moment the engine's own memory figure is reachable from here. Store
                    // it and re-derive replicator's slots: a mode already open was bounded by the
                    // per-machine cap alone, and the honest detent can only appear now.
                    self.instance_bytes = instance_bytes;
                    if self.replicator.is_some() {
                        let hosts = self.replicator_hosts();
                        let slots = logscope::replicator::slots(&hosts, instance_bytes);
                        if let Some(m) = self.replicator.as_mut() {
                            let before = m.want;
                            m.slots = slots;
                            // Shrink the fan if the real figure cannot hold what the cap allowed.
                            // Growing is NOT automatic: the operator chose that number, and a mode
                            // that adds workers on its own is a mode that starts work nobody asked
                            // for on a workspace it can edit.
                            let capped = before.min(m.slots.selectable_max());
                            if capped != before {
                                m.resize(capped);
                                self.note = format!("replicator: {before} -> {capped} worker(s) — {}",
                                    m.slots.explain(&model));
                            }
                        }
                    }
                    self.note = format!("lanes: {lanes} × {model} up — one weights copy, {lanes} KV caches; fans now decode {lanes} branches at once");
                    // Widen a fan that's already pool-driven (jobs queued before the load finished).
                    if !self.lane_jobs.is_empty() {
                        if let Some(f) = self.agent_fan.as_mut() { f.set_width(lanes); steps.extend(f.fill()); }
                    }
                }
                crate::lanes::LaneEvt::LoadError(e) => { self.error_banner = Some(e.to_string()); self.note = format!("lanes: load failed — {e}"); dead = true; }
                crate::lanes::LaneEvt::Token { job, text } => {
                    if let Some(&b) = self.lane_jobs.get(&job) {
                        if let Some(f) = self.agent_fan.as_mut() { f.on_token_branch(b, &text); }
                    }
                    // Replicator columns stream in place: the last line of a worker's lane IS its
                    // reply-in-progress, appended token by token, so the operator watches four
                    // workers think at once rather than waiting for four Dones.
                    if let Some(&w) = self.replica_jobs.get(&job) {
                        if let Some(m) = self.replicator.as_mut() {
                            if let Some(l) = m.lanes.get_mut(w) { l.append_token(&text); }
                        }
                    }
                }
                crate::lanes::LaneEvt::Done { job, text, .. } => {
                    if let Some(b) = self.lane_jobs.remove(&job) {
                        if let Some(f) = self.agent_fan.as_mut() { steps.extend(f.on_done_branch(b)); }
                    }
                    if let Some(w) = self.replica_jobs.remove(&job) {
                        if let Some(m) = self.replicator.as_mut() {
                            // Replace the streamed partial with the pool's final text: the two can
                            // differ (a lane may finish mid-token, and the pool's copy is what a
                            // partner would be quoted), and the partner-context path must quote the
                            // same words the worker actually produced.
                            if let Some(l) = m.lanes.get_mut(w) { l.finish_reply(&text); }
                        }
                    }
                }
                crate::lanes::LaneEvt::Error { job, err } => {
                    if let Some(w) = self.replica_jobs.remove(&job) {
                        if let Some(m) = self.replicator.as_mut() {
                            // The failure lands IN the column, not in a global banner: with four
                            // workers running, a banner cannot say which one died, and a column
                            // that simply stops looks identical to one still thinking.
                            if let Some(l) = m.lanes.get_mut(w) { l.finish_reply(&format!("⚠ {err}")); }
                        }
                    }
                    if let Some(b) = self.lane_jobs.remove(&job) {
                        if let Some(f) = self.agent_fan.as_mut() { steps.extend(f.on_error_branch(b, err)); }
                    }
                }
            }
        }
        if dead { self.lanes = None; self.lanes_ready = false; self.lane_jobs.clear(); }
        if !steps.is_empty() {
            let params = logscope::llm_backend::GenParams {
                max_tokens: self.ctx.max_tokens as usize, n_ctx: self.ctx.n_ctx, n_batch: self.ctx.n_ctx,
                temperature: self.ctx.temperature, ..Default::default()
            };
            if let Some(pool) = self.lanes.as_mut() {
                for s in steps {
                    if let crate::workspace_exec::FanStep::Submit { branch, prompt } = s {
                        let id = pool.submit(prompt, params.clone());
                        self.lane_jobs.insert(id, branch);
                    }
                }
            }
        }
        let fan_done = self.agent_fan.as_ref().map(|f| f.all_done()).unwrap_or(false);
        if fan_done && self.lane_jobs.is_empty() { self.finish_agent_fan(); }
    }

    fn finish_agent_fan(&mut self) {
        self.agent_grammar = None;
        let fan = match self.agent_fan.take() { Some(f) => f, None => return };
        let best = fan.best().map(|i| i + 1).unwrap_or(1);
        self.agent_candidates = fan.summaries();
        let verified = self.agent_candidates.iter().filter(|b| b.verified).count();
        let approved = self.agent_candidates.iter().filter(|b| b.approved == Some(true)).count();
        let n = self.agent_candidates.len();
        if self.fan_is_docs {
            self.note = format!("docs: {n} branches done — :merge-docs to union them into one organized tree");
        } else {
            self.note = format!("fan-agent: {n} branches · {approved} approved · {verified} verified · reviewer suggests branch {best} · :pick <n> to keep");
        }
    }

    /// Reveal a path in the desktop file manager (best-effort; Linux xdg-open, macOS open).
    fn reveal(path: &std::path::Path) {
        let prog = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let _ = std::process::Command::new(prog).arg(path).spawn();
    }

    /// Save artifacts to a chosen directory, remember it, and surface a note. Centralizes the
    /// three save paths (picker, re-save, zip) so they behave the same.
    fn save_to(&mut self, arts: &[crate::workspace::Artifact], dir: std::path::PathBuf) {
        self.ws_note = Some((match crate::workspace::write_to_dir(arts, &dir) {
            Ok(n) => { self.save_dir = Some(dir.clone()); self.last_saved = Some(dir.clone()); format!("saved {n} files → {}", dir.display()) }
            Err(e) => format!("error: {e}"),
        }, Instant::now()));
    }

    /// Pull an attached file into the composer as a fenced block (bounded), so the user can
    /// send a file as part of their message. Text only; large files are truncated with a note.
    fn attach(&mut self, path: &std::path::Path) {
        match std::fs::read(path) {
            Ok(bytes) => {
                let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
                let lang = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if !self.input.is_empty() && !self.input.ends_with('\n') { self.input.push('\n'); }
                let block = fenced_attachment(&name, lang, &bytes);
                self.input.push_str(&block);
            }
            Err(e) => { self.ws_note = Some((format!("attach error: {e}"), Instant::now())); }
        }
    }

    /// The spatial (3D) message view — messages as nodes in space, drag to orbit, scroll to zoom,
    /// click a node to focus it (preview below). Replaces the linear chat in aggressive mode. Renders
    /// via 2D projection of `spatial::layout` through `self.orbit`; the linear chat is left untouched.
    /// Returns `true` if the in-scene composer wants to send this frame (Enter pressed with non-empty
    /// input) — `spatial_ui` has no `Inference`/`Doc` to call `self.send()` itself, so the caller in `ui()`
    /// dispatches it.


    /// Modal key handling for the spatial view. Runs BEFORE the composer is built this frame (see the
    /// call in `ui`), which is what makes it reliable: in Normal mode we drain the character events here,
    /// so the message box — built later in the same frame — never sees the motion keys, regardless of
    /// whether it happens to hold focus. Insert/Command/Search are driven by their own widgets.
    fn handle_modal(&mut self, ui: &egui::Ui, infer: &Inference, doc: &crate::analysis::Doc) {
        use crate::vimmode::{Mode, Action};
        let composer = egui::Id::new("logscope_chat_composer");
        // Esc is the universal "back out": close the help overlay, drop any typing sub-mode to Normal, or
        // (already in Normal) cancel a pending z/leader prefix. Consume it so a focused box can't re-act.
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if self.show_help { self.show_help = false; return; }
            self.count_pending = None;
            match self.mode {
                Mode::Insert | Mode::Command | Mode::Search => {
                    self.mode = Mode::Normal;
                    self.cmdline.clear();
                    self.z_pending = false; self.leader_pending = false;
                    ui.ctx().memory_mut(|m| m.surrender_focus(composer));
                }
                Mode::Normal => { self.z_pending = false; self.leader_pending = false; }
            }
            return;
        }
        // Clicking into the message box is an implicit "I want to type" → Insert.
        if self.mode == Mode::Normal && ui.ctx().memory(|m| m.has_focus(composer)) { self.mode = Mode::Insert; }
        if self.mode != Mode::Normal { return; } // Insert/Command/Search are owned by their text widgets

        // Safety: if some OTHER text field (a side panel) holds focus, don't steal its keys.
        if let Some(f) = ui.ctx().memory(|m| m.focused()) { if f != composer { return; } }

        // Normal mode: the box must not hold focus, and no character may reach it. Because this runs before
        // the box is built, DRAINING the Text events is what actually stops the motion keys from typing.
        ui.ctx().memory_mut(|m| if m.has_focus(composer) { m.surrender_focus(composer); });
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) { self.enter_insert(ui); return; }
        let chars: Vec<(char, bool)> = ui.input_mut(|i| {
            let mut out = Vec::new();
            i.events.retain(|e| match e {
                egui::Event::Text(t) => { if let Some(c) = t.chars().next() { out.push((c.to_ascii_lowercase(), c.is_uppercase())); } false }
                _ => true,
            });
            out
        });
        for (c, shift) in chars {
            // Numeric prefix for motions (vim count): 21j, 6k, 3l. Digits accumulate; a leading 0 is
            // ignored (no count starts at 0). The count applies to the next repeatable motion, then resets.
            if c.is_ascii_digit() && !self.z_pending && !self.leader_pending {
                let d = c.to_digit(10).unwrap_or(0) as usize;
                if !(d == 0 && self.count_pending.is_none()) {
                    self.count_pending = Some(self.count_pending.unwrap_or(0).saturating_mul(10).saturating_add(d).min(100_000));
                }
                continue;
            }
            let act = crate::vimmode::normal_action(c, shift, self.z_pending, self.leader_pending);
            match act {
                Action::BeginZoomPrefix => { self.z_pending = true; self.leader_pending = false; }
                Action::BeginLeader => { self.leader_pending = true; self.z_pending = false; }
                Action::Help => { self.show_help = !self.show_help; self.z_pending = false; self.leader_pending = false; self.count_pending = None; }
                // jk scroll the reading plaque directly now (no prefix needed — hl took over the graph-
                // focus movement jk used to do; see vimmode::normal_action).
                Action::ScrollPreviewDown => { let max = self.plaque_max_line(); self.plaque_scroll.nudge(1.0, max); }
                Action::ScrollPreviewUp => { let max = self.plaque_max_line(); self.plaque_scroll.nudge(-1.0, max); }
                Action::None => { self.z_pending = false; self.leader_pending = false; self.count_pending = None; }
                _ => {
                    self.z_pending = false; self.leader_pending = false;
                    // repeatable motions honour the count; everything else runs once
                    let reps = match act {
                        Action::FocusNext | Action::FocusPrev | Action::SiblingNext | Action::SiblingPrev
                        | Action::ZoomIn | Action::ZoomOut => self.count_pending.take().unwrap_or(1).clamp(1, 10_000),
                        _ => { self.count_pending = None; 1 }
                    };
                    for _ in 0..reps { self.exec_action(act, ui, infer, doc); }
                }
            }
        }
    }

    fn exec_action(&mut self, act: crate::vimmode::Action, ui: &egui::Ui, _infer: &Inference, _doc: &crate::analysis::Doc) {
        use crate::vimmode::{Action, Mode, SearchKind};
        self.note.clear();
        match act {
            Action::FocusNext => self.move_focus(1),
            Action::FocusPrev => self.move_focus(-1),
            Action::SiblingNext => self.switch_sibling(1),
            Action::SiblingPrev => self.switch_sibling(-1),
            Action::First => self.set_focus(Some(0)),
            Action::Last => { let l = self.msgs.len().checked_sub(1); self.set_focus(l); }
            Action::ZoomIn => self.orbit.zoom(0.85),
            Action::ZoomOut => self.orbit.zoom(1.18),
            Action::Yaw(d) => self.orbit.orbit(d, 0.0),
            Action::Pitch(d) => self.orbit.orbit(0.0, d),
            Action::Search => { self.mode = Mode::Search; self.search_kind = SearchKind::Messages; self.search_q.clear(); }
            Action::TreeSearch => { self.mode = Mode::Search; self.search_kind = SearchKind::Tree; self.search_q.clear(); }
            Action::FuzzyTreeSearch => { self.mode = Mode::Search; self.search_kind = SearchKind::FuzzyTree; self.search_q.clear(); }
            Action::EnterCommand => { self.mode = Mode::Command; self.cmdline = ":".into(); }
            Action::EnterInsert => self.enter_insert(ui),
            Action::Branch => { if let Some(f) = self.walk_focus { self.branch_from = Some(f); } }
            Action::Overview => { self.spatial_auto = true; self.spatial_zoomed = false; self.walk_focus = None; }
            Action::ScrollPreviewDown => { let max = self.plaque_max_line(); self.plaque_scroll.nudge(1.0, max); }
            Action::ScrollPreviewUp => { let max = self.plaque_max_line(); self.plaque_scroll.nudge(-1.0, max); }
            Action::BeginZoomPrefix | Action::BeginLeader | Action::Help | Action::None => {}
        }
        self.spatial_zoomed = self.spatial_zoomed || matches!(act, Action::ZoomIn | Action::ZoomOut);
        ui.ctx().request_repaint();
    }

    fn enter_insert(&mut self, ui: &egui::Ui) {
        self.mode = crate::vimmode::Mode::Insert;
        ui.ctx().memory_mut(|m| m.request_focus(egui::Id::new("logscope_chat_composer")));
    }

    /// Focus a node: highlight it, recenter the camera on it, and (for the linear view) jump-scroll.
    fn set_focus(&mut self, id: Option<usize>) {
        self.walk_focus = id;
        if let Some(i) = id {
            self.scroll_chat_to = Some(i);
            if let Some(p) = self.fold_pos.get(i).copied() { self.orbit.target = p; self.spatial_auto = false; }
        }
    }

    /// Move focus along the live branch (j/k): +1 toward the leaf, −1 toward the root.
    fn move_focus(&mut self, dir: i32) {
        let g = self.convo_graph(self.msgs.len());
        let path = g.active_path();
        if path.is_empty() { return; }
        let cur = self.walk_focus.and_then(|f| path.iter().position(|&x| x == f));
        let next = match cur {
            Some(p) => (p as i32 + dir).clamp(0, path.len() as i32 - 1) as usize,
            None => if dir < 0 { path.len() - 1 } else { 0 },
        };
        self.set_focus(Some(path[next]));
    }

    /// The highest scrollable start-line for the physical-UI reading plaque of whichever message is
    /// currently focused — the top of the visible window can go no further than `total lines - PLAQUE_WINDOW`.
    /// Used both by the jk scroll handlers (before the plaque renders this frame) and by the renderer
    /// itself, so the two always agree on the same wrap.
    fn plaque_max_line(&self) -> f32 {
        let focus = std::env::var("LOGSCOPE_FOCUS").ok().and_then(|s| s.trim().parse::<usize>().ok()).or(self.walk_focus);
        match focus.filter(|i| *i < self.msgs.len()) {
            Some(fid) => {
                let raw = self.msgs[fid].text().to_string();
                let all_lines = wrap_label(&raw, 24, usize::MAX);
                all_lines.len().saturating_sub(PLAQUE_WINDOW) as f32
            }
            None => 0.0,
        }
    }

    /// Switch focus to a sibling branch (h/l) and make that branch live (its lineage becomes the prompt).
    fn switch_sibling(&mut self, dir: i32) {
        let Some(f) = self.walk_focus else { self.move_focus(dir); return; };
        let g = self.convo_graph(self.msgs.len());
        let parent = g.nodes.get(f).and_then(|n| n.parent);
        let sibs: Vec<usize> = g.nodes.iter().filter(|n| n.parent == parent).map(|n| n.id).collect();
        if sibs.len() <= 1 { return; }
        let idx = sibs.iter().position(|&x| x == f).unwrap_or(0);
        let ni = (idx as i32 + dir).rem_euclid(sibs.len() as i32) as usize;
        let sib = sibs[ni];
        // follow first-children to a leaf so the switched-to branch becomes the live lineage
        let mut tip = sib;
        while let Some(c) = g.nodes.iter().find(|n| n.parent == Some(tip)) { tip = c.id; }
        self.active_leaf = Some(tip);
        self.set_focus(Some(sib));
    }

    /// `:a <path>` — read a file (bounded) and stage it into the composer, then drop into Insert.
    /// Hosts available to replicator mode.
    ///
    /// Today that is THIS MACHINE ONLY, reported honestly rather than optimistically: the fleet
    /// side (fleet::sightings over the DHT) can enumerate peers, but nothing yet ships a model
    /// instance to one and streams it back, so counting remote machines here would offer slots the
    /// mode cannot fill. The seam is `Vec<Host>` precisely so that adding peers later is a change
    /// to this function and nothing else.
    fn replicator_hosts(&self) -> Vec<logscope::replicator::Host> {
        // `resident_budget_bytes` lives in the engine crate, which the default build does not link
        // (wgpu-engine is opt-in) — this call un-gated is what broke `cargo check` with no features.
        // Without the engine, `instance_bytes` never leaves 0 (it is set from the lane pool's Ready
        // event), and `replicator::slots` reads per-instance 0 as "no memory bound" — so the budget
        // is never consulted and 0 is inert rather than a lie. Duplicating the meminfo parse here to
        // fake a figure would just be a second copy of the rule waiting to drift.
        #[cfg(feature = "wgpu-engine")]
        let budget = gpu_kernels::store::resident_budget_bytes().unwrap_or(0);
        #[cfg(not(feature = "wgpu-engine"))]
        let budget = 0u64;
        vec![logscope::replicator::Host { id: "local".into(), budget_bytes: budget }]
    }

    /// What one more instance of the loaded model would cost, learned from the lane pool's Ready
    /// event (weights the planner placed + a KV cache at the default window).
    ///
    /// 0 until a pool has loaded, and `replicator::slots` reads 0 as "no memory bound" rather than
    /// as zero slots — so before :lanes the slider shows the full per-machine track and after it
    /// the detent appears where the device actually stops. A GGUF file-size proxy was rejected for
    /// the gap: a q4 file on disk is not what int8 residency costs, and a slider that promises four
    /// workers and OOMs on the third is worse than one that stops at the sane cap.
    fn per_instance_bytes(&self) -> u64 { self.instance_bytes }

    fn model_label(&self) -> String {
        self.stamp_model.clone()
            .or_else(|| self.local_model_path.as_ref().and_then(|p| {
                std::path::Path::new(p).file_stem().map(|s| s.to_string_lossy().into_owned())
            }))
            .unwrap_or_else(|| "the model".into())
    }

    /// Start a `:image` generation on a worker thread.
    ///
    /// This is the trigger the whole sd-cli path was missing: mediagen builds the argv and
    /// media::generate_image runs it, both tested, and NOTHING called them — the feature was
    /// complete except for the part that starts it.
    ///
    /// The binary comes from LOGSCOPE_SD_CLI, or `sd-cli` on PATH. Resolution failures are
    /// reported the way the fleet reports refusals: named, in the transcript, with the thing to
    /// fix — a silent no-op here would be indistinguishable from a broken build.
    /// `:vision <image> <prompt>` — the image-IN half of the media pipeline.
    ///
    /// `model_kind` has recognised a vision projector since it was written and its guidance
    /// ends at "load the VLM and point --mmproj at this", leaving the user to work out WHICH
    /// vlm from a directory of similarly-named files. `visionpair` does the pairing; this is
    /// the command that uses it.
    ///
    /// Staged as a note rather than spawned, which is what `:image` earned the hard way: the
    /// argv is pure and pinned by tests, and a command you can read before it takes the GPU
    /// for a minute is worth more than one that already started.
    fn start_vision(&mut self, args: &str) {
        let mut it = args.trim().splitn(2, char::is_whitespace);
        let (Some(img), Some(prompt)) = (it.next().filter(|s| !s.is_empty()), it.next()) else {
            self.note = "usage: :vision <image> <prompt> \u{2014} e.g. :vision shot.png what is on the screen".into();
            return;
        };
        let models = logscope::llm_backend::default_models_dir();
        let files: Vec<std::path::PathBuf> = std::fs::read_dir(&models)
            .map(|d| d.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        let pairs = crate::visionpair::pairs(&files);
        let Some(pair) = pairs.into_iter().next() else {
            // NAMED, not just refused. "no vision model" leaves someone unable to tell
            // whether they have the wrong files or the wrong directory.
            self.note = format!(
                ":vision needs a model with its mmproj beside it in {} \u{2014} found none. A projector                  alone is not enough: it pairs with the model it was trained with, and this refuses to                  guess.",
                models.display()
            );
            return;
        };
        let bin = std::env::var_os("LOGSCOPE_MTMD_CLI")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("llama-mtmd-cli"));
        let run = crate::visionpair::VisionRun::new(bin, pair, std::path::PathBuf::from(img), prompt);
        self.note = format!("would run: {}", run.argv().join(" "));
    }

    fn start_image(&mut self, prompt: &str) {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            self.note = "usage: :image <prompt> — e.g. :image a red bicycle in the rain".into();
            return;
        }
        if self.image_job.is_some() {
            self.note = "an image is already generating — one at a time (sd-cli wants the GPU too)".into();
            return;
        }
        let sd_cli = std::env::var_os("LOGSCOPE_SD_CLI").map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("sd-cli"));
        let models = logscope::llm_backend::default_models_dir();
        // Named by prompt and clock so two generations never collide, and so the file is
        // identifiable later without opening it.
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0);
        let slug: String = prompt.chars().filter(|c| c.is_ascii_alphanumeric() || *c == ' ')
            .take(40).collect::<String>().trim().replace(' ', "-").to_lowercase();
        let out = self.save_dir.clone().unwrap_or_else(std::env::temp_dir)
            .join(format!("logscope-{stamp}-{}.png", if slug.is_empty() { "image".into() } else { slug }));
        let (tx, rx) = std::sync::mpsc::channel();
        let (p2, models2) = (prompt.clone(), models.clone());
        std::thread::spawn(move || {
            let _ = tx.send(crate::media::generate_image(sd_cli, &models2, &p2, out));
        });
        self.image_job = Some(ImageJob { rx, prompt, started: Instant::now(), result: None, tex: None, tex_err: None });
    }

    /// Send one composed message into the fan: compose per-worker prompts, submit each to the lane
    /// pool, and remember which job belongs to which column.
    ///
    /// Lanes are the right vehicle and were already built: LanePool runs N generators over ONE
    /// resident weight set with N KV caches, which is exactly the shape replicator's memory bound
    /// assumes. Without a pool this falls back to reporting what it WOULD have sent, because a
    /// mode that silently drops the operator's message is worse than one that admits it is
    /// half-wired.
    fn dispatch_replicator(&mut self, text: &str) {
        let params = self.params();
        let broadcast_once = std::mem::take(&mut self.fan_broadcast_once);
        let Some(m) = &mut self.replicator else { return };
        let aim = if broadcast_once { None } else { m.aim };
        let jobs = m.submit_to(text, aim);
        if jobs.is_empty() {
            self.note = "replicator: nothing aimed at — the column you targeted is gone".into();
            return;
        }
        match self.lanes.as_mut() {
            Some(pool) => {
                for d in &jobs {
                    let id = pool.submit(d.prompt.clone(), params.clone());
                    self.replica_jobs.insert(id, d.to);
                }
                let crossed = jobs.iter().filter(|d| d.had_context).count();
                self.note = format!("replicator: {} worker(s) working{}", jobs.len(),
                    if crossed > 0 { format!(", {crossed} with partner context") } else { String::new() });
            }
            None => {
                self.note = format!("replicator: composed for {} worker(s) but no lane pool is up \
— run :lanes {} first (one weights copy, {} KV caches)", jobs.len(), jobs.len(), jobs.len());
            }
        }
    }

    /// Draw replicator mode's panel when it is on. Nothing at all when it is off — a mode's UI
    /// should not cost a frame it is not part of.
    fn draw_replicator(&mut self, ui: &mut egui::Ui) {
        let model = self.model_label();
        let Some(m) = &mut self.replicator else { return };
        let (slots, want, aim) = (m.slots.clone(), m.want, m.aim);
        let mut views: Vec<crate::replicatorui::ColumnView<'_>> = m.workers.iter()
            .zip(m.lanes.iter_mut())
            .map(|(w, l)| crate::replicatorui::ColumnView {
                id: w.id, host: w.host.as_str(), turn: l.turn, streaming: l.streaming,
                // The assignment edits IN PLACE through the view; the panel owns no copy that
                // could go stale between frames.
                assignment: &mut l.assignment,
                lines: &l.lines[..],
            }).collect();
        let act = crate::replicatorui::panel(ui, &slots, want, aim, &mut views, &m.grid,
                                             &m.crossings, &model);
        drop(views);
        if let Some(a) = act.aim { m.aim = a; }
        if act.toggle_matrix { m.show_matrix = !m.show_matrix; }
        if let Some(v) = act.want { m.resize(v); }

        if m.show_matrix {
            let labels: Vec<String> = m.workers.iter().zip(m.lanes.iter())
                .map(|(w, l)| if l.assignment.is_empty() { format!("w{}", w.id) } else { l.assignment.clone() })
                .collect();
            // The turn a link is set on ANCHORS its cadence, so the matrix needs the clock: the
            // newest lane turn is the operator's "now".
            let now = m.lanes.iter().map(|l| l.turn).max().unwrap_or(0) as u32;
            crate::replicatorui::dilation_matrix(ui, &mut m.grid, &labels, now);
        }
        // Applied AFTER the `m` borrow ends: Enter in an assignment field aimed the composer, and
        // this hands it the keyboard so the operator types the task immediately instead of
        // hunting for the box. (Setting self.mode inside the borrow does not compile — the same
        // shape as poll_image's deferred dismissal.)
        if act.focus_composer {
            ui.ctx().memory_mut(|mm| mm.request_focus(egui::Id::new("logscope_chat_composer")));
            self.mode = crate::vimmode::Mode::Insert;
        }
    }

    /// Poll the running generation and surface it. Called once per frame from the chat UI.
    ///
    /// Dismissal is collected as a flag and applied AFTER the job borrow ends: an egui closure that
    /// clears `self.image_job` while the match still holds `&mut` on it does not compile, and the
    /// tempting fix (clone the job into the closure) would copy the Receiver out of the field it
    /// has to stay in.
    fn poll_image(&mut self, ui: &mut egui::Ui) {
        let Some(job) = &mut self.image_job else { return };
        if job.result.is_none() {
            if let Ok(r) = job.rx.try_recv() { job.result = Some(r); }
        }
        let mut dismiss = false;
        match &job.result {
            None => {
                let (elapsed, prompt) = (job.started.elapsed().as_secs_f32(), job.prompt.clone());
                ui.horizontal(|ui| {
                    theme::pulse_dot(ui, theme::CYAN());
                    ui.label(RichText::new(format!("generating an image… ({elapsed:.0}s) — {prompt}"))
                        .color(theme::CYAN()).small());
                });
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
            Some(Ok(path)) => {
                let path = path.clone();
                // Decode + upload ONCE, on the first frame after the generation lands. decode.rs
                // has handled every colour type since it was written and had no callers until now;
                // the picture appearing in the transcript is what it was for.
                if job.tex.is_none() && job.tex_err.is_none() {
                    match std::fs::read(&path).map_err(|e| e.to_string())
                        .and_then(|b| crate::decode::decode_png_rgba(&b))
                    {
                        Ok((rgba, w, h)) => {
                            let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                            job.tex = Some(ui.ctx().load_texture(
                                format!("genimage::{}", path.display()), img, egui::TextureOptions::LINEAR));
                        }
                        Err(e) => job.tex_err = Some(e),
                    }
                }
                let tex = job.tex.clone();
                let tex_err = job.tex_err.clone();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("🖼 {}", path.display())).color(theme::GREEN()).small());
                    if theme::carved_button(ui, "open").clicked() { Self::reveal(&path); }
                    if theme::carved_button(ui, "dismiss").clicked() { dismiss = true; }
                });
                if let Some(e) = tex_err {
                    ui.label(RichText::new(format!("(written, but not displayable here: {e})"))
                        .color(theme::AMBER()).small());
                }
                if let Some(tex) = tex {
                    // Fit the WIDTH and keep the aspect: a 1024² at native size overflows a phone
                    // and pushes the composer off screen — the same out-of-bounds class
                    // mobilelayout::dialog_width fixes for dialogs.
                    let avail = ui.available_width().max(64.0);
                    let [tw, th] = tex.size();
                    let w = avail.min(tw as f32);
                    let h = w * th as f32 / (tw as f32).max(1.0);
                    ui.add(egui::Image::new(&tex).fit_to_exact_size(egui::vec2(w, h)));
                }
            }
            Some(Err(e)) => {
                // Every failure mode here is actionable — no binary on PATH or in LOGSCOPE_SD_CLI,
                // no diffusion model under models/image, no text encoder under models/text_encoders
                // — so the message is the whole error rather than a summary of it.
                let e = e.clone();
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("image generation failed: {e}")).color(theme::RED()).small());
                    if theme::carved_button(ui, "dismiss").clicked() { dismiss = true; }
                });
            }
        }
        if dismiss { self.image_job = None; }
    }

    fn attach_path(&mut self, path: &str, ui: &egui::Ui) {
        let p = shellexpand_home(path);
        match std::fs::read(&p) {
            Ok(bytes) => {
                let name = std::path::Path::new(&p).file_name().and_then(|s| s.to_str()).unwrap_or("file");
                let lang = std::path::Path::new(&p).extension().and_then(|e| e.to_str()).unwrap_or("");
                if !self.input.is_empty() && !self.input.ends_with('\n') { self.input.push('\n'); }
                let block = fenced_attachment(name, lang, &bytes);
                self.input.push_str(&block);
                self.enter_insert(ui);
            }
            Err(e) => { self.cmdline = format!(":a — can't read {path}: {e}"); }
        }
    }

    /// Build the whole conversation tree as export nodes (role + text + parent), for treeexport.
    fn build_export_nodes(&self) -> Vec<crate::treeexport::ExportNode> {
        let g = self.convo_graph(self.msgs.len());
        g.nodes.iter().map(|nd| crate::treeexport::ExportNode {
            id: nd.id,
            parent: nd.parent,
            role: if nd.turn == crate::convograph::Turn::User { "you".into() } else { "assistant".into() },
            text: self.msgs.get(nd.id).map(|m| m.text().to_string()).unwrap_or_default(),
        }).collect()
    }

    fn write_export(&self, content: &str, ext: &str) -> Option<std::path::PathBuf> {
        let dir = autosave_path()?.parent()?.join("exports");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("tree-{}.{ext}", now_unix()));
        std::fs::write(&path, content).ok()?;
        Some(path)
    }

    fn modal_hint(&self) -> String {
        if self.z_pending { return "z →  h/l yaw · k/j pitch · Esc cancel".into(); }
        if self.leader_pending { return "leader →  / tree-search · \\ fuzzy-tree · Esc cancel".into(); }
        if self.spatial_view {
            "j/k scroll message · h/l move · H/L branch · g/G ends · i/I zoom · z+hjkl rotate · o fit · / find · : cmd · b branch · a write · ? help".into()
        } else {
            "j/k scroll message · h/l move · H/L branch · g/G ends · / find · : cmd · b branch · a write · ? help".into()
        }
    }

    fn run_command(&mut self, cmd: crate::vimmode::Command, ui: &egui::Ui) {
        use crate::vimmode::Command;
        match cmd {
            // The `:irc` family. Parsed by logscope-fan's vimmode and dispatched by
            // logscope-fan's cmdpalette; this SHELL has not been wired to the consent
            // gate yet, so each arm says so by name rather than doing nothing.
            //
            // Silence would be the wrong failure here specifically. The whole point of
            // `irc-kit`'s design is that adding a channel log costs two deliberate steps
            // — `:irc <net>/<chan>` prints the consent text and arms a pending ask,
            // `:ircy <chan>` confirms by typing the channel name back — precisely so
            // muscle memory cannot grant access to other people's writing. A command
            // that appears to be accepted and quietly grants nothing teaches the
            // operator that the gate is noise, which is the one lesson it must not
            // teach. So: name the command, name what is missing.
            Command::IrcStatus => {
                self.note = ":irc — channel-log sources are not wired into this shell yet \
(shared/irc-kit has the log index and the consent gate; logscope-fan's ircsrc.rs is the seam)".into();
            }
            Command::IrcAsk(chan) => {
                self.note = format!(
                    ":irc {chan} — not wired into this shell yet, so NOTHING has been granted. \
When it is, this prints the consent text and arms an ask that `:ircy {chan}` confirms.");
            }
            Command::IrcConfirm(chan) => {
                self.note = format!(
                    ":ircy {chan} — there is no pending ask to confirm; the `:irc` family is not \
wired into this shell yet.");
            }
            Command::IrcRevoke(chan) => {
                // Revocation reads as safe to no-op and is not: an operator who believes
                // a grant was withdrawn will act on that belief.
                self.note = format!(
                    ":ircx {chan} — nothing to withdraw. Channel-log sources are not wired into \
this shell, so no grant exists here to revoke.");
            }

            Command::IrcFind(q) => {
                // Search is the reach the grant gates, so an unwired search must not
                // read as "no results" — that is indistinguishable from a granted log
                // containing nothing.
                self.note = format!(
                    ":ircf {q} — channel-log search is not wired into this shell yet. This is NOT \
an empty result set: nothing was searched.");
            }

            Command::Branch => { if let Some(f) = self.walk_focus { self.branch_from = Some(f); } }
            Command::Attach(p) => self.attach_path(&p, ui),
            Command::Image(prompt) => self.start_image(&prompt),
            Command::Vision(args) => self.start_vision(&args),
            Command::Goto(n) => {
                let i = n.saturating_sub(1).min(self.msgs.len().saturating_sub(1));
                // Glide along the graph's shortest path (like clicking a node) AND zoom in, so a voice "go to
                // 12" for an off-screen node visibly travels there and arrives prominent — set_focus alone only
                // re-centres at the current (far overview) distance, which read as "nothing happened".
                let from = self.walk_focus.unwrap_or(i);
                let g = self.convo_graph(self.msgs.len());
                let path = graph_path(&g, from, i);
                if path.len() > 1 { self.cam_pan_path = path; self.cam_pan_t0 = Some(std::time::Instant::now()); }
                self.set_focus(Some(i));
                self.orbit.dist = self.orbit.dist.clamp(8.0, 13.0); // pull in from overview so the node reads
                self.spatial_auto = false; self.spatial_zoomed = true;
            }
            Command::Prune => { if let Some(f) = self.walk_focus { self.pruned.insert(f); self.fold_pos.clear(); } }
            Command::Unprune => { if let Some(f) = self.walk_focus { self.pruned.remove(&f); } }
            Command::Overview => { self.spatial_auto = true; self.spatial_zoomed = false; self.walk_focus = None; }
            Command::Linear => { self.set_view(false, ui); }
            Command::Spatial => { self.set_view(true, ui); }
            Command::Export(fmt) => {
                let nodes = self.build_export_nodes();
                let (content, ext) = match fmt.as_str() {
                    "ascii" | "txt" | "tree" => (crate::treeexport::ascii(&nodes), "txt"),
                    "git" | "sh" => (crate::treeexport::git_script(&nodes), "sh"),
                    _ => (crate::treeexport::markdown(&nodes), "md"),
                };
                self.note = match self.write_export(&content, ext) {
                    Some(p) => format!("exported tree ({fmt}) → {}", p.display()),
                    None => "export failed (no exports dir)".into(),
                };
            }
            // :fuzz N — fan N framing-perturbed variants of the composer text as branches off the focus,
            // queued to generate one after another (uses the fuzz + queue + branching pieces together).
            Command::Fuzz(n) => {
                let base = self.base_for_fan();
                if !base.is_empty() {
                    let parent = self.walk_focus.or(self.active_leaf);
                    for v in crate::consensus::fuzz(&base, n, now_unix()) { self.queue.push_back((v, parent)); }
                    self.input.clear();
                    self.note = format!("fuzz: fanned {n} retries (:stop to cancel)");
                } else { self.note = ":fuzz — nothing to fan (type a prompt, or send one first)".into(); }
            }
            // :fuzzd N — like :fuzz, but each branch carries a different BEHAVIORAL directive (strategy/
            // persona/constraint…) instead of a lexical reword, so the fan explores different approaches
            // to the same task. The directive name could later label each branch in the spatial view.
            Command::FuzzDirective(n) => {
                let base = self.base_for_fan();
                if !base.is_empty() {
                    let parent = self.walk_focus.or(self.active_leaf);
                    for (v, _name) in crate::directives::fanout_user(&base, n, now_unix()) { self.queue.push_back((v, parent)); }
                    self.input.clear();
                    self.note = format!("fuzzd: fanned {n} directive-varied branches (:stop to cancel)");
                } else { self.note = ":fuzzd — nothing to fan (type a prompt, or send one first)".into(); }
            }
            // :o / :open <path> — import a chat (markdown ## You / ## Assistant) and load it as the convo.
            Command::Open(p) => {
                if p.trim().is_empty() { self.note = ":open needs a path (e.g. :o chat.md)".into(); }
                else {
                    let path = shellexpand_home(p.trim());
                    match std::fs::read_to_string(&path) {
                        Ok(text) => {
                            let turns = parse_imported_chat(&text);
                            if turns.is_empty() { self.note = format!(":open — no ## You / ## Assistant turns found in {p}"); }
                            else { let n = turns.len(); self.load_imported(turns); self.note = format!("imported {n} turns from {p} — :ca to fan, :fuzzd to branch"); }
                        }
                        Err(e) => { self.note = format!(":open — can't read {p}: {e}"); }
                    }
                }
            }
            // :ca [N] / :Nca — fan a continuation on every leaf, N rounds deep. fan_step kicks round 1;
            // the queue drain re-fires it on the new frontier for the remaining rounds.
            Command::ContinueAll(depth) => {
                if self.leaf_ids().is_empty() { self.note = "continue-all — no leaves yet".into(); }
                else {
                    self.fan = Some(Fan { kind: FanKind::Continue, rounds_left: depth, only: None });
                    self.fan_step();
                    if depth > 1 { self.note = format!("continue-all depth {depth} — fanning the frontier (:stop to cancel)"); }
                }
            }
            // :fa [W] ['tmpl'] / :Nfa W 'tmpl' — depth×width templated fuzzy fan. The task is the template
            // (placeholders {lastmsg}/{msgid} filled per leaf) or, if blank, the composer text. Each round
            // fans `width` fuzzed variants per leaf; `depth` rounds deepen the frontier.
            Command::FuzzyAll { depth, width, template } => {
                let task = if template.trim().is_empty() { self.input.trim().to_string() } else { template };
                if task.is_empty() { self.note = ":fa needs a template or composer text — e.g. :4fa 10 'redo, not like {lastmsg}'".into(); }
                else if self.leaf_ids().is_empty() { self.note = ":fa — no leaves yet (send a turn or :open a chat first)".into(); }
                else {
                    self.input.clear();
                    self.fan = Some(Fan { kind: FanKind::Fuzzy { width, template: task }, rounds_left: depth, only: None });
                    self.fan_step();
                    self.note = format!("fuzzy-all depth {depth} × width {width} — fanning (:stop to cancel)");
                }
            }
            Command::Stop => {
                let n = self.queue.len(); self.queue.clear(); self.fan = None;
                if let Some(p) = &self.lanes { p.cancel_all(); }
                self.lane_jobs.clear();
                self.agent_grammar = None;
                if self.agent_run.take().is_some() || self.agent_fan.take().is_some() {
                    self.agent_pending_submit = None;
                    self.pending_cancel = true; // stop the in-flight generation too
                    self.note = "agent run cancelled".into();
                } else {
                    self.note = format!("stopped — cleared {n} queued, fan cancelled");
                }
            }
            // :continue [N] / :cont — deepen the FOCUSED line N times. Unlike :ca (every leaf), this pins the
            // fan to the current line via `only`, and fan_step descends to that line's live tip each round.
            Command::Continue(n) => {
                match self.walk_focus.or(self.active_leaf).or(self.msgs.len().checked_sub(1)) {
                    Some(anchor) => {
                        self.fan = Some(Fan { kind: FanKind::Continue, rounds_left: n, only: Some(anchor) });
                        self.fan_step();
                        self.note = format!("continue ×{n} — deepening this line (:stop to cancel)");
                    }
                    None => self.note = ":continue — nothing to continue yet (send a turn first)".into(),
                }
            }
            // :ws / :workspace — status of the loaded /tmp .gitmind session and how much has changed in it.
            // :ws <path> — open a workspace, which is the same operation :load-gitmind performs;
            // reusing it rather than a second loader means one mirroring path, one set of failure
            // messages, and no chance of the two drifting.
            Command::OpenWorkspace(path) => {
                self.run_command(crate::vimmode::Command::LoadGitmind(path), ui);
                if self.gitmind_session.is_some() {
                    self.note = format!("{} · :replicate to fan workers across it", self.note);
                }
            }
            // :replicate [n] — enter replicator mode. The count is BOUNDED here rather than
            // trusted: the slider's ceiling is a hardware fact (replicator::slots), and a typed
            // ":replicate 12" on a machine with room for 3 must land on 3 with a reason, not
            // start twelve workers that thrash.
            Command::Replicate(n) => {
                let hosts = self.replicator_hosts();
                let per_instance = self.per_instance_bytes();
                let s = logscope::replicator::slots(&hosts, per_instance);
                let want = n.unwrap_or(s.selectable_max().min(3));
                let got = want.clamp(s.min, s.selectable_max());
                self.replicator = Some(ReplicatorMode::new(&s, got));
                self.note = if got == want {
                    format!("replicator: {got} worker(s) — {}", s.explain(&self.model_label()))
                } else {
                    format!("replicator: {got} worker(s), not {want} — {}", s.explain(&self.model_label()))
                };
            }
            Command::WorkspaceStatus => {
                self.note = match &self.gitmind_session {
                    None => ":ws — no workspace loaded (:load-gitmind <path> to mirror one into /tmp)".into(),
                    Some(s) => {
                        let changed = crate::gitmind_session::changed_files(s.work_dir(), &s.project);
                        let head: String = changed.iter().take(6).cloned().collect::<Vec<_>>().join(", ");
                        format!("workspace '{}' → {} · {} changed file(s){}", s.name, s.work_dir().display(),
                                changed.len(), if head.is_empty() { String::new() } else { format!(": {head}") })
                    }
                };
            }
            // :sync — rescan the /tmp workspace (pick up edits made since load); reports the new change count.
            Command::SyncWorkspace => {
                self.note = match &self.gitmind_session {
                    None => ":sync — no workspace loaded (:load-gitmind <path> first)".into(),
                    Some(s) => {
                        let n = crate::gitmind_session::changed_files(s.work_dir(), &s.project).len();
                        format!("workspace rescanned — {n} changed file(s) (:map to fan work per file, :reapply to land)")
                    }
                };
            }
            // :map <glob> ['tmpl'] — fan one worker task per changed file in the /tmp workspace, filtered by an
            // optional glob, with {file} filled into the template. The per-file fan-out for a software project.
            Command::MapFiles { glob, template } => {
                match &self.gitmind_session {
                    None => self.note = ":map — no workspace loaded (:load-gitmind <path> first)".into(),
                    Some(s) => {
                        let mut files = crate::gitmind_session::changed_files(s.work_dir(), &s.project);
                        if !glob.trim().is_empty() { files.retain(|f| crate::vimmode::glob_match(&glob, f)); }
                        if files.is_empty() {
                            self.note = format!(":map — no {}changed files in the workspace",
                                                if glob.trim().is_empty() { String::new() } else { format!("'{glob}' ") });
                        } else {
                            let parent = self.walk_focus.or(self.active_leaf);
                            let tasks = crate::vimmode::map_tasks(&files, &template);
                            let n = tasks.len();
                            for t in tasks { self.queue.push_back((t, parent)); }
                            self.note = format!(":map — fanned {n} worker task(s), one per changed file (:stop to cancel)");
                        }
                    }
                }
            }
            // :fill [path] — structured decomposition. Take a skeleton (path, else the composer if it holds
            // a todo!(), else the latest reply), find its todo!()/unimplemented!() holes, and fan one
            // sub-task per hole. Every sub-task embeds the SAME skeleton → the KV prefix tree prefills it
            // once and each sub-task is a cheap divergent suffix (1 prefill + N completions).
            Command::Fill(path) => {
                let from_composer = path.trim().is_empty()
                    && (self.input.contains("todo!()") || self.input.contains("unimplemented!()"));
                let skeleton = if !path.trim().is_empty() {
                    match std::fs::read_to_string(shellexpand_home(path.trim())) {
                        Ok(s) => s,
                        Err(e) => { self.note = format!(":fill — can't read {path}: {e}"); String::new() }
                    }
                } else if from_composer {
                    self.input.trim().to_string()
                } else {
                    let raw = self.msgs.iter().rev().find(|m| m.role == Role::Assistant).map(|m| m.text().to_string()).unwrap_or_default();
                    crate::decompose::extract_code_block(&raw).unwrap_or(raw) // a reply's skeleton is usually fenced
                };
                if skeleton.is_empty() {
                    if self.note.is_empty() { self.note = ":fill needs a skeleton — paste one with todo!() bodies, give :fill <path>, or generate one first".into(); }
                } else {
                    let holes = crate::decompose::find_holes(&skeleton);
                    if holes.is_empty() { self.note = ":fill — found no todo!()/unimplemented!() holes in the skeleton".into(); }
                    else {
                        let parent = self.walk_focus.or(self.active_leaf);
                        for h in &holes { self.queue.push_back((crate::decompose::subtask_prompt(&skeleton, h), parent)); }
                        let n = holes.len();
                        self.fill_session = Some(FillSession { skeleton, holes });
                        if from_composer { self.input.clear(); }
                        self.note = format!("fill: {n} sub-tasks queued — all share the skeleton prefix (KV-reused); :assemble when they finish");
                    }
                }
            }
            // :assemble — splice the filled bodies back into the skeleton. Maps the last N assistant replies
            // (N = hole count) onto the holes positionally, in order; result goes to the composer to review.
            Command::Assemble => {
                if let Some(fs) = self.fill_session.clone() {
                    let n = fs.holes.len();
                    let bodies: Vec<String> = {
                        let mut v: Vec<String> = self.msgs.iter().rev().filter(|m| m.role == Role::Assistant)
                            .take(n).map(|m| crate::decompose::extract_body(m.text())).collect();
                        v.reverse();
                        v
                    };
                    if bodies.len() < n { self.note = format!(":assemble — only {} of {n} sub-task replies present; let the fan finish first", bodies.len()); }
                    else {
                        let fills: Vec<(crate::decompose::Hole, String)> = fs.holes.iter().cloned().zip(bodies).collect();
                        self.input = crate::decompose::assemble(&fs.skeleton, &fills);
                        self.note = format!("assembled {n} bodies into the composer — review/save (spliced over the todo!()s)");
                    }
                } else { self.note = ":assemble — run :fill first to set up a skeleton".into(); }
            }
            // :quest [N] — enqueue a clarifying-questions pass; the reply is the question list to answer.
            Command::Quest(n) => {
                let task = self.msgs.iter().rev().find(|m| m.role == Role::User).map(|m| m.text().to_string()).unwrap_or_default();
                if task.is_empty() { self.note = ":quest — ask something first".into(); }
                else {
                    let context: Vec<String> = self.msgs.iter().filter(|m| m.role == Role::Assistant).map(|m| m.text().to_string()).collect();
                    let parent = self.walk_focus.or(self.active_leaf);
                    self.queue.push_back((crate::quest::quest_prompt(&task, n, &context), parent));
                    self.note = format!("asking {n} clarifying questions — answer the A: lines, then :ans");
                }
            }
            // :ans — turn a filled Q/A block (pasted in the composer) into a focused context to send.
            Command::Answer(_path) => {
                let pairs = crate::quest::parse_answers(&self.input);
                if pairs.is_empty() { self.note = ":ans — paste the filled Q/A block into the composer first".into(); }
                else { self.input = crate::quest::clarified_context(&pairs); self.note = format!("folded {} decisions into the composer — send to generate", pairs.len()); }
            }
            // :check — the compile oracle lives in logscope-cli (a synchronous compile would block the UI
            // thread); the GUI keeps the code-block proxy for :best.
            Command::Check => { self.note = ":check (compile oracle) runs in logscope-cli; the GUI uses the code-block proxy for :best".into(); }
            Command::Explore => { self.note = ":explore (exploratory compilation over the fan) runs in logscope-cli — it needs the compile oracle (rustc/cargo check)".into(); }
            // :best — the summary agent's pick: highlight + focus the subtree that verified best.
            Command::Best => {
                let parents = self.effective_parents();
                let verdicts = self.node_verdicts();
                match crate::bestregion::best_region(&parents, &verdicts, 2, 0.5) {
                    Some(r) => {
                        let (root, n, depth) = (r.root, r.members.len(), r.max_depth);
                        self.region = Some(r.members.into_iter().collect());
                        self.set_focus(Some(root));
                        self.spatial_auto = false;
                        self.note = format!("best region: {n} nodes from #{root} (depth {depth}) — code-producing; wire the cargo oracle for real verdicts");
                    }
                    None => { self.region = None; self.note = "best region: nothing qualifies yet — grow the space (:ca / :fuzzd) so there's code to judge".into(); }
                }
            }
            // :use — model classes are a logscope-cli feature; the GUI chat tab drives a single backend.
            Command::Use(_) => { self.note = ":use (model classes) runs in logscope-cli — the GUI chat tab drives a single model".into(); }
            Command::MergeCtx { refs, instruction } => {
                let (ids, bad) = crate::vimmode::resolve_refs(self.msgs.len(), &refs);
                if ids.is_empty() {
                    self.note = format!(":mergectx — no ids resolved{}", if bad.is_empty() { String::new() } else { format!(" ({} unknown)", bad.len()) });
                } else {
                    // Flatten the referenced turns into one candidate list and stage it in the composer, so you
                    // can switch to a bigger model and send "pick the best" against the shortlist in one shot.
                    let mut merged = String::from("Below are candidate responses, each from a different branch.\n");
                    for (k, &i) in ids.iter().enumerate() {
                        let t = self.msgs.get(i).map(|m| m.text().trim().to_string()).unwrap_or_default();
                        merged.push_str(&format!("\n## Candidate {} · id {}\n{}\n", k + 1, crate::vimmode::short_id(i), t));
                    }
                    let instr = if instruction.trim().is_empty() {
                        "Pick the single best candidate. Begin with its id, then say why it wins and where the others fall short."
                    } else { instruction.trim() };
                    merged.push_str(&format!("\n---\n{instr}"));
                    self.input = merged;
                    self.note = format!("merged {} nodes into the composer — pick a model and send", ids.len());
                }
            }
            Command::Strategy { refs, focus } => {
                let ids: Vec<usize> = if refs.is_empty() { (0..self.msgs.len()).collect() }
                    else { crate::vimmode::resolve_refs(self.msgs.len(), &refs).0 };
                if ids.is_empty() { self.note = ":strategy — nothing to summarize".into(); }
                else {
                    let mut transcript = String::new();
                    for &i in &ids {
                        if let Some(m) = self.msgs.get(i) {
                            let role = if m.role == Role::User { "User" } else { "Assistant" };
                            let t: String = m.text().chars().take(600).collect();
                            transcript.push_str(&format!("\n[{role} · {}] {}\n", crate::vimmode::short_id(i), t.trim()));
                        }
                    }
                    let mut body = String::from(crate::vimmode::STRATEGIST_PROMPT);
                    if !focus.trim().is_empty() { body.push_str(&format!("\n\nFocus especially on: {}", focus.trim())); }
                    body.push_str(&format!("\n\n--- CONVERSATION ({} turns) ---\n{transcript}", ids.len()));
                    self.input = body;
                    self.note = format!("bird's-eye over {} turns staged — pick a model and send", ids.len());
                }
            }
            Command::Segment { refs, focus } => {
                let ids: Vec<usize> = if refs.is_empty() { (0..self.msgs.len()).collect() }
                    else { crate::vimmode::resolve_refs(self.msgs.len(), &refs).0 };
                if ids.is_empty() { self.note = ":segment — nothing to map".into(); }
                else {
                    let mut transcript = String::new();
                    for (pos, &i) in ids.iter().enumerate() {
                        if let Some(m) = self.msgs.get(i) {
                            let role = if m.role == Role::User { "User" } else { "Assistant" };
                            let t: String = m.text().chars().take(500).collect();
                            transcript.push_str(&format!("\n[#{} {role} · {}] {}\n", pos + 1, crate::vimmode::short_id(i), t.trim()));
                        }
                    }
                    let mut body = String::from(crate::vimmode::SEGMENTER_PROMPT);
                    if !focus.trim().is_empty() { body.push_str(&format!("\n\nPay special attention to: {}", focus.trim())); }
                    body.push_str(&format!("\n\n--- TRANSCRIPT ({} turns) ---\n{transcript}", ids.len()));
                    self.input = body;
                    self.note = format!("block-map over {} turns staged — pick a model and send", ids.len());
                }
            }
            Command::Todo { .. } => { self.note = ":todo (small-model TODO scan → weak links) runs in logscope-cli".into(); }
            Command::Dump(path) => {
                let root = shellexpand_home(if path.trim().is_empty() { "." } else { path.trim() });
                // the active lineage (tip → root), reversed to chronological
                let mut chain = Vec::new();
                let mut cur = self.active_leaf;
                while let Some(i) = cur { chain.push(i); cur = self.msgs.get(i).and_then(|m| m.parent); }
                chain.reverse();
                // EVERY BRANCH, EVERY MESSAGE, EVERY VARIANT. This walked active_leaf back through
                // parents and wrote that ONE chain — dropping every alternative a fan-out produced, and
                // within each surviving turn every regeneration except the selected one. It reported the
                // turns it saved, so nothing indicated what was missing — the worst shape a data-loss bug
                // can have.
                let all_branches = logscope::branchtree::branches(&self.msgs, self.active_leaf);
                let alt_variants: usize =
                    self.msgs.iter().map(|m| m.all_variants().len().saturating_sub(1)).sum();
                if all_branches.len() > 1 || alt_variants > 0 {
                    eprintln!("export: {}", logscope::branchtree::manifest(&all_branches));
                    eprintln!("export: {alt_variants} alternative variant(s) beyond the selected ones");
                }
                let turns: Vec<(String, String)> = chain.iter().filter_map(|&i| self.msgs.get(i))
                    .map(|m| ((if m.role == Role::Assistant { "assistant" } else { "user" }).to_string(), m.text().to_string()))
                    .collect();
                if turns.is_empty() { self.note = ":dump — nothing to save yet".into(); }
                else {
                    let id = format!("chat-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
                    let title = chain.iter().find_map(|&i| self.msgs.get(i)).map(|m| m.text().chars().take(60).collect::<String>()).unwrap_or_default();
                    let dump = crate::chatdump::ChatDump::from_turns(&id, title, turns);
                    match dump.export_to_workspace(std::path::Path::new(&root)) {
                        Ok(dir) => self.note = format!("📦 dumped {} turns → {}", dump.turns.len(), dir.display()),
                        Err(e) => self.note = format!(":dump — failed: {e}"),
                    }
                }
            }
            Command::LoadChat(dir) => {
                let dir = shellexpand_home(dir.trim());
                match crate::chatdump::ChatDump::load(std::path::Path::new(&dir)) {
                    Ok(d) => {
                        let turns: Vec<(Role, String)> = d.turns.iter()
                            .map(|t| (if t.role == "assistant" { Role::Assistant } else { Role::User }, t.text.clone())).collect();
                        let n = turns.len();
                        self.load_imported(turns);
                        self.note = format!("📂 loaded “{}” — {n} turns from {dir}", d.title);
                    }
                    Err(e) => self.note = format!(":loadchat — can't read {dir}: {e}"),
                }
            }
            Command::ImportWorkspace(dir) => {
                let dir = shellexpand_home(dir.trim());
                let found = crate::chatdump::import_dir(std::path::Path::new(&dir));
                if found.is_empty() { self.note = format!(":import-workspace — no logscope/chats dumps found under {dir}"); }
                else {
                    let ids: Vec<String> = found.iter().take(8).map(|d| d.id.clone()).collect();
                    self.note = format!("🗂 found {} chat dump(s): {} — open one with :lc <dir>", found.len(), ids.join(", "));
                }
            }
            Command::SummarizeFiles(_glob) => {
                // stage the chat's files into the sandbox mirror, then (with a model) ask for per-file summaries
                self.note = ":summarize-files stages the chat's files into the sandbox; per-file summaries need a loaded model".into();
            }
            Command::LoadGitmind(path) => {
                let p = path.trim();
                if p.is_empty() {
                    self.note = ":load-gitmind needs a project path (e.g. :lgm ~/work/monorepo)".into();
                } else {
                    let expanded = match p.strip_prefix("~/") {
                        Some(rest) => std::env::var("HOME").map(|h| format!("{h}/{rest}")).unwrap_or_else(|_| p.to_string()),
                        None => p.to_string(),
                    };
                    let proj = std::path::Path::new(&expanded);
                    let session = proj.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "session".into());
                    let tmp = std::env::temp_dir().join("logscope-gitmind");
                    match crate::gitmind_session::load_gitmind(proj, &session, &tmp) {
                        Ok(s) => {
                            self.note = format!("loaded '{}' → editing {} · :reapply to land it as a commit", session, s.work_dir().display());
                            self.gitmind_session = Some(s);
                        }
                        Err(e) => self.note = format!(":load-gitmind — {e}"),
                    }
                }
            }
            Command::Agent(task) => {
                // NON-BLOCKING: the driver runs over the session's isolated /tmp workspace; the Evt
                // loop advances it turn by turn, and :stop / barge-in interrupt it.
                self.start_agent_task(task.trim().to_string(), ":agent");
            }
            Command::FanAgent(task) => {
                let task = task.trim().to_string();
                let mut cfg = crate::workspace_exec::AgentConfig::default();
                if let Some(line) = self.stage_lane(&mut cfg, &task) { self.hud.observe(line, "delegator"); }
                self.start_fan(&task, cfg, false, "fan-agent");
            }
            Command::Lanes(arg) => {
                let a = arg.trim();
                if a.eq_ignore_ascii_case("off") || a == "0" {
                    if let Some(p) = self.lanes.take() { p.shutdown(); }
                    self.lane_jobs.clear(); self.lanes_ready = false;
                    self.note = "lanes: off — fans round-robin on the single worker again".into();
                } else {
                    let n = a.parse::<usize>().ok().filter(|&n| (1..=8).contains(&n)).unwrap_or(2);
                    match self.local_model_path.clone() {
                        None => self.note = ":lanes — pick a LOCAL .gguf in the model list first (lanes share its resident weights)".into(),
                        Some(path) => {
                            if let Some(p) = self.lanes.take() { p.shutdown(); }
                            self.lane_jobs.clear(); self.lanes_ready = false;
                            let name = std::path::Path::new(&path).file_name()
                                .map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
                            self.lanes = Some(crate::lanes::LanePool::spawn(path, n));
                            self.note = format!("lanes: loading {n} lane(s) over {name} — one weights copy, one KV cache per lane…");
                        }
                    }
                }
            }
            Command::DocFan(task) => {
                // A DOC fan: each branch writes a slice of the docs under the organization convention, so the
                // outputs merge cleanly (:merge-docs). Doc mode = convention in the prompt, no code-verify.
                let task = task.trim().to_string();
                let mut cfg = crate::workspace_exec::AgentConfig::docs();
                if let Some(line) = self.stage_lane(&mut cfg, &task) { self.hud.observe(line, "delegator"); }
                self.start_fan(&task, cfg, true, "docs");
            }
            Command::MergeDocs => self.merge_fan_docs(),
            Command::Pick(n) => self.pick_branch(n),
            Command::Reapply => match self.gitmind_session.take() {
                None => self.note = ":reapply — no .gitmind session loaded (:load-gitmind <path> first)".into(),
                Some(s) => match crate::gitmind_session::reapply(&s) {
                    Ok(hash) => self.note = format!("reapplied '{}' → commit {}", s.name, &hash[..hash.len().min(10)]),
                    Err(e) => {
                        self.note = format!(":reapply — {e}");
                        self.gitmind_session = Some(s); // keep the session so the user can retry
                    }
                },
            },
            Command::Template => {
                match logscope::llm_backend::active_model() {
                    None => self.note = ":template — no model loaded".into(),
                    Some(m) => {
                        let path = m.path.clone();
                        match template_kit::gguf::read_templates_from(std::path::Path::new(&path)) {
                            Err(e) => self.note = format!(":template — {e}"),
                            Ok(t) => {
                                let arch = t.architecture.clone().unwrap_or("?".into());
                                match t.for_tools() {
                                    None => self.note = format!(":template — {arch}: this GGUF carries no chat template; the hand-built {:?} family is the only option", self.live_template()),
                                    Some(src) => match template_kit::Renderer::new(src) {
                                        Err(e) => self.note = format!(":template — {arch}: the GGUF's template does not compile here: {e}"),
                                        Ok(r) => {
                                            // Render the SAME two turns both ways and show the diff in one line
                                            // plus the full text in the transcript, so the choice is visible.
                                            let msgs = vec![template_kit::Message::system("S"), template_kit::Message::user("U")];
                                            let own = r.render(&msgs, &[], true).unwrap_or_else(|e| format!("(render failed: {e})"));
                                            // The comparison uses the SAME synthetic turns on both
                                            // sides (system "S", user "U"), so what differs is the
                                            // template and nothing else. The live history would make
                                            // the two sides incomparable.
                                            let built = self.render_family_sample(self.live_template(), "S", "U");
                                            let syntax = r.probe_tool_syntax().ok();
                                            let fmt = template_kit::toolcall::detect_format(src);
                                            let flag = std::env::var("LOGSCOPE_GGUF_TEMPLATE").as_deref() == Ok("1");
                                            self.push_note_block(&format!(
                                                "## template · {arch}\n\nGGUF template: {} chars, tool syntax {:?}, call format {:?}\nLOGSCOPE_GGUF_TEMPLATE is {}\n\n### the model's own, rendered\n```\n{}\n```\n\n### the hand-built {:?} family, rendered\n```\n{}\n```\n",
                                                src.len(), syntax.as_ref().map(|s| s.call_open.clone()).unwrap_or("(no tool support)".into()), fmt,
                                                if flag { "ON (the model's own is in use)" } else { "off (the hand-built family is in use)" },
                                                own, self.live_template(), built.chars().take(1200).collect::<String>()));
                                            self.note = format!("template: {arch} · {:?} · see the transcript", fmt);
                                        }
                                    },
                                }
                            }
                        }
                    }
                }
            }
            Command::Survey(q) => {
                let q = q.trim().to_string();
                if q.is_empty() { self.note = ":survey needs a question".into(); }
                else {
                    // The workspace under survey: the loaded session's scratch, else the cwd.
                    let root = self.gitmind_session.as_ref().map(|s| s.work_dir().to_path_buf()).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    let ceiling: u32 = std::env::var("LOGSCOPE_SURVEY_TOKENS").ok().and_then(|s| s.parse().ok()).unwrap_or(6_000);
                    let sess = crate::queen::answer(&q, ceiling, |p| crate::probe_exec::run(&root, p).map_err(|e| format!("{e:?}")));
                    let absences = crate::queen::record_absences(&sess, &root.display().to_string());
                    let plan = crate::queen::to_plan(&q, &sess, if self.workers.is_empty() { None } else { Some(&self.workers) });
                    let files = plan.tasks.len().saturating_sub(1);
                    self.note = format!("survey: {} probes, {} findings, {} misses → {} absence(s) ledgered · plan: {} read task(s) + answer{} · Hive → Run plan",
                        sess.ran.len(), sess.findings.len(), sess.misses.len(), absences, files, if sess.budget_exhausted { " (budget exhausted)" } else { "" });
                    self.last_plan = Some((q, plan));
                }
            }
            Command::Bundle => match self.gitmind_session.as_ref() {
                None => self.note = ":bundle — no .gitmind session loaded (:load-gitmind <path> first)".into(),
                Some(s) => {
                    // Snapshot first so the bundle carries everything on disk, then export. The
                    // exports dir is per user, not per project: the project tree is never written.
                    let snapped = crate::mount::snapshot(&s.ws, &format!("logscope: {}", s.name)).unwrap_or(false);
                    let out = std::env::var("LOGSCOPE_EXPORTS").map(std::path::PathBuf::from)
                        .unwrap_or_else(|_| std::env::var("XDG_DATA_HOME").map(std::path::PathBuf::from)
                            .unwrap_or_else(|_| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share")).join("logscope").join("exports"));
                    match crate::mount::bundle(&s.ws, &out) {
                        Ok(a) => {
                            self.note = format!("bundle: {} · {} file(s){} · Hive → Integrate, or `git fetch {} {}`",
                                a.bundle.display(), a.changed.len(), if snapped { " (snapshotted)" } else { "" }, a.bundle.display(), s.ws.branch);
                            self.last_bundle = Some(a.bundle);
                        }
                        Err(e) => self.note = format!(":bundle — {e}"),
                    }
                }
            },
            Command::Compact(arg) => {
                // active lineage (tip → root), reversed to chronological
                let mut chain = Vec::new();
                let mut cur = self.active_leaf;
                while let Some(i) = cur { chain.push(i); cur = self.msgs.get(i).and_then(|m| m.parent); }
                chain.reverse();
                // EVERY BRANCH, EVERY MESSAGE, EVERY VARIANT. This walked active_leaf back through
                // parents and wrote that ONE chain — dropping every alternative a fan-out produced, and
                // within each surviving turn every regeneration except the selected one. It reported the
                // turns it saved, so nothing indicated what was missing — the worst shape a data-loss bug
                // can have.
                let all_branches = logscope::branchtree::branches(&self.msgs, self.active_leaf);
                let alt_variants: usize =
                    self.msgs.iter().map(|m| m.all_variants().len().saturating_sub(1)).sum();
                if all_branches.len() > 1 || alt_variants > 0 {
                    eprintln!("export: {}", logscope::branchtree::manifest(&all_branches));
                    eprintln!("export: {alt_variants} alternative variant(s) beyond the selected ones");
                }
                let turns: Vec<(String, String)> = chain.iter().filter_map(|&i| self.msgs.get(i))
                    .map(|m| ((if m.role == Role::Assistant { "assistant" } else { "user" }).to_string(), m.text().to_string()))
                    .collect();
                if turns.len() < 2 { self.note = ":compact — need at least two turns of history to compact".into(); }
                else {
                    let ratio = arg.trim().parse::<f32>().ok().filter(|r| *r > 0.0 && *r < 1.0).unwrap_or(0.3);
                    let plan = crate::refresh::plan(&turns, 4, ratio);
                    // Guard on the SAVING, not the turn count. `turns.len() < 2`
                    // above lets a two-turn chat fold one turn: a field report
                    // shows a seven-token message staged with "Target: about 6
                    // tokens", i.e. a whole generation to recover nothing.
                    if let Some(why) = plan.refusal() { self.note = why; return; }
                    let folded: Vec<(String, String)> = turns[..plan.fold].to_vec();
                    self.input = crate::refresh::teacher_turn(&folded, plan.target_tokens);
                    self.note = format!(
                        "🗜 compaction staged: fold {} turns → ~{} tok, keep {} recent → context {}→{} tok (−{:.0}%). \
                         The briefing becomes a pinned KV checkpoint reused by every branch below it. Pick a model and send.",
                        plan.fold, plan.target_tokens, plan.keep, plan.before_tokens, plan.after_tokens, plan.ratio_saved() * 100.0
                    );
                }
            }
            Command::Race(arg) => {
                let mut chain = Vec::new();
                let mut cur = self.active_leaf;
                while let Some(i) = cur { chain.push(i); cur = self.msgs.get(i).and_then(|m| m.parent); }
                chain.reverse();
                // EVERY BRANCH, EVERY MESSAGE, EVERY VARIANT. This walked active_leaf back through
                // parents and wrote that ONE chain — dropping every alternative a fan-out produced, and
                // within each surviving turn every regeneration except the selected one. It reported the
                // turns it saved, so nothing indicated what was missing — the worst shape a data-loss bug
                // can have.
                let all_branches = logscope::branchtree::branches(&self.msgs, self.active_leaf);
                let alt_variants: usize =
                    self.msgs.iter().map(|m| m.all_variants().len().saturating_sub(1)).sum();
                if all_branches.len() > 1 || alt_variants > 0 {
                    eprintln!("export: {}", logscope::branchtree::manifest(&all_branches));
                    eprintln!("export: {alt_variants} alternative variant(s) beyond the selected ones");
                }
                let turns: Vec<(String, String)> = chain.iter().filter_map(|&i| self.msgs.get(i))
                    .map(|m| ((if m.role == Role::Assistant { "assistant" } else { "user" }).to_string(), m.text().to_string()))
                    .collect();
                if turns.len() < 2 { self.note = ":race — need at least two turns of history first".into(); }
                else {
                    let ratio = arg.trim().parse::<f32>().ok().filter(|r| *r > 0.0 && *r < 1.0).unwrap_or(0.3);
                    let plan = crate::refresh::plan(&turns, 4, ratio);
                    self.note = format!(
                        "🏁 race plan: full ≈ {} tok vs compacted ≈ {} tok (−{:.0}%). Run :compact to produce the briefing, then both run off the same parent — keep whichever wins, or flag a bad jump with :why.",
                        plan.before_tokens, plan.after_tokens, plan.ratio_saved() * 100.0
                    );
                }
            }
            Command::Diagnose(refs) => {
                let pick = if refs.len() >= 2 {
                    let (ids, _) = crate::vimmode::resolve_refs(self.msgs.len(), &refs);
                    if ids.len() >= 2 { Some((ids[0], ids[1])) } else { None }
                } else {
                    // default: the active leaf (the compacted path you're on) vs a sibling sharing its parent (the full run)
                    self.active_leaf.and_then(|leaf| {
                        let parent = self.msgs.get(leaf).and_then(|m| m.parent);
                        (0..self.msgs.len())
                            .find(|&j| j != leaf && self.msgs.get(j).and_then(|m| m.parent) == parent)
                            .map(|sib| (sib, leaf))
                    })
                };
                match pick {
                    Some((fi, ci)) => {
                        let fo = self.msgs.get(fi).map(|m| m.text().to_string()).unwrap_or_default();
                        let co = self.msgs.get(ci).map(|m| m.text().to_string()).unwrap_or_default();
                        if fo.trim().is_empty() || co.trim().is_empty() { self.note = ":why — both turns need generated output to compare".into(); }
                        else {
                            self.input = crate::race::diagnose_turn(&[], "", &fo, &co);
                            let div = crate::race::divergence(&fo, &co);
                            self.request_smaller_model = true; // ui() switches to the smallest local model
                            self.note = format!("🚩 bad jump flagged — the two runs diverge {:.0}%. Spawning the smallest local model to diagnose; send when ready.", div * 100.0);
                        }
                    }
                    None => { self.note = ":why — give two turns to compare, e.g. :why <full-id>,<compacted-id>".into(); }
                }
            }
            Command::Capacity => {
                let tier = logscope::capacity_tree::detect_tier();
                let tree = logscope::capacity_tree::capacity_tree();
                let mut lines = logscope::capacity_tree::briefing_for_tier(&tree, tier);
                if logscope::capacity_tree::physical_ram_gib().is_none() {
                    lines.push("  (RAM not detected on this platform — showing the middle tier; \
run :capacity on the target machine for a real reading)".into());
                }
                self.note = lines.join("\n");
            }
            Command::Queen(task) => {
                let task = task.trim();
                if task.is_empty() {
                    self.note = format!(":queen needs a task — {} worker(s) on the roster already ({})",
                        self.roster.len(), self.roster.names().join(", "));
                } else {
                    self.input = self.roster.queen_request(task);
                    self.note = "👑 staged the Queen's prompt — pick a model and send; \
:enlist the reply once it comes back.".into();
                }
            }
            Command::Enlist => { self.note = self.do_enlist(); }
            Command::Help(arg) => {
                let a = arg.trim();
                if a.is_empty() {
                    self.note = format!("{} commands · :help <name> for usage. Common: compact, race, why, suggest, fuzz, use, dump, branch, best.", crate::cmdpalette::SPECS.len());
                } else if let Some(s) = crate::cmdpalette::spec(a) {
                    self.note = format!("{}  —  {}   (e.g. {})", s.usage, s.help, s.example);
                } else {
                    let near: Vec<&str> = crate::cmdpalette::matches(a).iter().take(5).map(|s| s.name).collect();
                    self.note = if near.is_empty() { format!("no command matches “{a}” — :help to list them") }
                        else { format!("no exact “{a}”. Did you mean: {}?", near.join(", ")) };
                }
            }
            Command::Tour(arg) => {
                // The TOGGLE resolves here, not in the parser: only the tab knows whether the tour
                // is currently running, and a parser that guessed would make a bare ":tour" mean
                // "on" forever. Some(0.0) is an explicit off; any other speed enables at that pace.
                let was = self.fly_auto;
                self.fly_auto = match arg {
                    None => if was.is_some() { None } else { Some(logscope_fan::vimmode::TOUR_DEFAULT_SPEED) },
                    Some(v) if v <= 0.0 => None,
                    Some(v) => Some(v),
                };
                // Leaving freecam on would fight the tour for the camera every frame.
                if self.fly_auto.is_some() { self.freecam = false; }
                self.note = match self.fly_auto {
                    Some(v) => format!(":tour ON at {v:.2}x — flying the strand; :tour off to stop"),
                    None => ":tour OFF — camera released".into(),
                };
            }
            Command::Suggest => {
                let mut chain = Vec::new();
                let mut cur = self.active_leaf;
                while let Some(i) = cur { chain.push(i); cur = self.msgs.get(i).and_then(|m| m.parent); }
                chain.reverse();
                let recent: Vec<(String, String)> = chain.iter().rev().take(3).rev().filter_map(|&i| self.msgs.get(i))
                    .map(|m| ((if m.role == Role::Assistant { "assistant" } else { "user" }).to_string(), m.text().to_string()))
                    .collect();
                if recent.is_empty() { self.note = ":suggest — no messages to read yet".into(); }
                else {
                    let names: Vec<&str> = crate::cmdpalette::SPECS.iter().map(|s| s.name).collect();
                    self.input = crate::prompts::suggester_input(&recent, &names);
                    self.request_smaller_model = true;
                    self.note = "💡 the smallest local model will read your last 3 messages and suggest commands — send to run.".into();
                }
            }
            Command::Prompt(arg) => {
                let key = arg.trim();
                if key.is_empty() {
                    self.note = format!("prompts: {} · :prompt <key> to stage one", crate::prompts::keys().join(", "));
                } else if let Some(p) = crate::prompts::by_key(key) {
                    let mut chain = Vec::new();
                    let mut cur = self.active_leaf;
                    while let Some(i) = cur { chain.push(i); cur = self.msgs.get(i).and_then(|m| m.parent); }
                    chain.reverse();
                    let mut body = String::from(p.body);
                    let recent: Vec<usize> = chain.iter().rev().take(6).rev().copied().collect();
                    if !recent.is_empty() {
                        body.push_str("\n\n--- TEXT ---\n");
                        for i in recent {
                            if let Some(m) = self.msgs.get(i) {
                                let role = if m.role == Role::Assistant { "Assistant" } else { "User" };
                                let t: String = m.text().chars().take(800).collect();
                                body.push_str(&format!("\n[{role}] {}\n", t.trim()));
                            }
                        }
                    }
                    self.input = body;
                    self.note = format!("📝 staged “{}” ({}) — pick a model and send.", p.title, p.summary);
                } else {
                    self.note = format!("no prompt “{key}” — :prompt to list them");
                }
            }
            Command::Noop | Command::Unknown(_) => {}
        }
    }

    /// The prominent, always-visible modal indicator (both views). A colour-coded mode pill, then either
    /// the live `:`/`/` input (Command/Search) or a contextual key hint + focus position + a `?` for the
    /// full keybinding overlay. This sits at the top of the chat area — the old indicator was an 18px
    /// strip pinned below the composer and was easy to miss, which is why the modes felt invisible.
    fn modal_banner(&mut self, ui: &mut egui::Ui) {
        use crate::vimmode::{Mode, SearchKind};
        let mc = { let (r, g, b) = self.mode.rgb(); egui::Color32::from_rgb(r, g, b) };
        ui.horizontal(|ui| {
            theme::chip(ui, self.mode.label(), mc);
            ui.add_space(5.0);
            match self.mode {
                Mode::Command => {
                    let r = ui.add(egui::TextEdit::singleline(&mut self.cmdline).id(egui::Id::new("vim_cmdline"))
                        .desired_width(f32::INFINITY).font(egui::TextStyle::Small));
                    // Only when it does NOT already have focus. request_focus() on a
                    // focused TextEdit re-initialises its cursor state every frame, so
                    // the caret jumps back and each keystroke overwrites the last —
                    // the "one character" bug. The composer at the top of this file
                    // already guards it this way; these two did not.
                    if !r.has_focus() { r.request_focus(); }
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let cmd = crate::vimmode::parse_command(&self.cmdline);
                        let keep = matches!(cmd, crate::vimmode::Command::Attach(_)); // :a drops into Insert
                        self.run_command(cmd, ui);
                        if !keep { self.mode = Mode::Normal; }
                        self.cmdline.clear();
                    }
                    // live palette hint: complete the command, show its usage, and suggest arguments
                    let raw = self.cmdline.trim_start_matches(':');
                    let mut parts = raw.splitn(2, char::is_whitespace);
                    let head = parts.next().unwrap_or("");
                    let arg_partial = parts.next().unwrap_or("");
                    if let Some(s) = crate::cmdpalette::spec(head) {
                        ui.label(RichText::new(format!("{}  ·  {}", s.usage, s.help)).color(theme::DIM()).small());
                        let ctx = self.arg_ctx();
                        let sugg = crate::cmdpalette::suggest_args(s, arg_partial, &ctx);
                        if !sugg.is_empty() {
                            let line = sugg.iter().take(4).map(|a| format!("{} — {}", a.value, a.hint)).collect::<Vec<_>>().join("    ");
                            ui.label(RichText::new(line).color(theme::FAINT()).small());
                        }
                    } else if !head.is_empty() {
                        let names: Vec<&str> = crate::cmdpalette::matches(head).iter().take(8).map(|s| s.name).collect();
                        if !names.is_empty() {
                            ui.label(RichText::new(names.join("   ")).color(theme::DIM()).small());
                        } else {
                            ui.label(RichText::new("no match · :help to list commands").color(theme::FAINT()).small());
                        }
                    }
                }
                Mode::Search => {
                    let k = match self.search_kind { SearchKind::Messages => "/", SearchKind::Tree => "tree/", SearchKind::FuzzyTree => "fuzzy/" };
                    ui.label(RichText::new(k).color(theme::DIM()).small());
                    let r = ui.add(egui::TextEdit::singleline(&mut self.search_q).id(egui::Id::new("vim_search"))
                        .desired_width(280.0).font(egui::TextStyle::Small));
                    // Same guard as the command line: unconditional request_focus()
                    // resets the cursor every frame and eats all but one character.
                    if !r.has_focus() { r.request_focus(); }
                    if !self.search_q.is_empty() {
                        let previews: Vec<(usize, String)> = self.msgs.iter().enumerate()
                            .map(|(i, m)| (i, m.text().chars().take(80).collect())).collect();
                        let ranked = crate::vimmode::fuzzy_rank(&self.search_q, previews.iter().map(|(i, s)| (*i, s.as_str())));
                        if let Some(&(best, _)) = ranked.first() { self.set_focus(Some(best)); }
                        ui.label(RichText::new(format!("{} hits · Enter keeps · Esc cancels", ranked.len())).color(theme::FAINT()).small());
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) { self.mode = Mode::Normal; }
                }
                _ => {
                    if let Some(c) = self.count_pending {
                        ui.label(RichText::new(format!("{c}×")).color(theme::CYAN()).small().strong());
                    }
                    if !self.note.is_empty() {
// THE PERSISTENT BANNER, above the transient note. A load failure carries the numbers an
// operator needs — host RAM, resident, page cache, the flag that would help — and it used to
// land in `note`, which the next status overwrites.
//
// Copy comes first because the first thing anyone does with an error is paste it somewhere.
// Close is explicit: an error that dismisses itself on a timer is one that vanishes while
// being read, and this text is long enough to still be reading.
if let Some(msg) = self.error_banner.clone() {
    egui::Frame::NONE
        .fill(theme::RED().linear_multiply(0.12))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("model not loaded").color(theme::RED()).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").on_hover_text("dismiss").clicked() {
                        self.error_banner = None;
                    }
                    if ui.small_button(uikit_egui::icons::copy_label(ui.ctx())).on_hover_text("copy the full message").clicked() {
                        ui.ctx().copy_text(msg.clone());
                    }
                });
            });
            // Wrapped rather than truncated: the useful part of these messages is the numbers at
            // the end, so eliding the tail removes exactly what someone opened it for.
            ui.label(RichText::new(&msg).small());
        });
    ui.add_space(4.0);
}
                        ui.label(RichText::new(&self.note).color(theme::GREEN()).small());
                    } else {
                        ui.label(RichText::new(self.modal_hint()).color(theme::FAINT()).small());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("?").on_hover_text("keybindings (or press ?)").clicked() { self.show_help = !self.show_help; }
                        if let Some(f) = self.walk_focus {
                            ui.label(RichText::new(format!("turn {}/{}", f + 1, self.msgs.len())).color(theme::DIM()).small());
                        }
                        // long-horizon: the active system persona — now click-to-switch inline (was edit-only
                        // on the Context page). Switching sets self.context via the persona_by_key seam.
                        let pbtn = ui.small_button(RichText::new(self.active_persona_label()).color(theme::FAINT()).small())
                            .on_hover_text("system persona steering replies — click to switch");
                        let ppid = ui.make_persistent_id("persona_popup");
                        if pbtn.clicked() { ui.memory_mut(|m| m.toggle_popup(ppid)); }
                        egui::popup_below_widget(ui, ppid, &pbtn, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                            ui.set_min_width(210.0);
                            ui.label(RichText::new("system persona").color(theme::DIM()).small());
                            for (key, title, _) in PERSONAS {
                                let active = self.active_persona_label() == *title;
                                if ui.selectable_label(active, RichText::new(*title).small()).on_hover_text(*key).clicked() {
                                    if let Some(body) = persona_by_key(key) { self.context = body.to_string(); }
                                }
                            }
                        });
                        // fan commands, surfaced in the GUI: a live HUD + stop while work is in flight, else a
                        // launcher so the frontier can be fanned without dropping to the `:` cmdline.
                        if let Some(act) = self.activity_status() {
                            if ui.small_button("⏹").on_hover_text("stop — clear the queue and cancel the fan (:stop)").clicked() {
                                let n = self.queue.len(); self.queue.clear(); self.fan = None;
                                self.note = format!("stopped — cleared {n} queued, fan cancelled");
                            }
                            ui.label(RichText::new(act).color(theme::CYAN()).small().strong());
                        } else if !self.msgs.is_empty() {
                            use crate::vimmode::Command;
                            let fbtn = ui.small_button(RichText::new("⑃ fan").color(theme::DIM()).small())
                                .on_hover_text("fan the frontier — continue or fuzz every leaf (:ca · :fa · :fuzz)");
                            let fpid = ui.make_persistent_id("fan_popup");
                            if fbtn.clicked() { ui.memory_mut(|m| m.toggle_popup(fpid)); }
                            let mut launch: Option<Command> = None;
                            egui::popup_below_widget(ui, fpid, &fbtn, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                                ui.set_min_width(220.0);
                                ui.label(RichText::new("fan the frontier").color(theme::DIM()).small());
                                if ui.selectable_label(false, RichText::new("continue every leaf").small()).on_hover_text(":ca").clicked() { launch = Some(Command::ContinueAll(1)); }
                                if ui.selectable_label(false, RichText::new("continue · depth 3").small()).on_hover_text(":3ca").clicked() { launch = Some(Command::ContinueAll(3)); }
                                if ui.selectable_label(false, RichText::new("fuzz ×8 (this turn)").small()).on_hover_text(":fuzz 8 — needs composer text").clicked() { launch = Some(Command::Fuzz(8)); }
                                if ui.selectable_label(false, RichText::new("fuzzy-all ×4").small()).on_hover_text(":fa 4").clicked() { launch = Some(Command::FuzzyAll { depth: 1, width: 4, template: String::new() }); }
                            });
                            if let Some(cmd) = launch { self.run_command(cmd, ui); }
                        }
                    });
                }
            }
        });
        ui.add_space(2.0);
    }

    /// `?` overlay: the full motion map, grouped. Dismiss with `?`, Esc, or clicking away.
    fn help_overlay(&mut self, ui: &egui::Ui) {
        if !self.show_help { return; }
        let spatial = self.spatial_view;
        egui::Window::new("keybindings")
            .collapsible(false).resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                let row = |ui: &mut egui::Ui, k: &str, d: &str| {
                    ui.horizontal(|ui| {
                        ui.add_sized([86.0, 14.0], egui::Label::new(RichText::new(k).color(theme::CYAN()).monospace().small()).wrap());
                        ui.label(RichText::new(d).color(theme::INK()).small());
                    });
                };
                ui.label(RichText::new("MOVE").color(theme::DIM()).small().strong());
                row(ui, "j / k", "focus down / up the live branch");
                row(ui, "h / l", "switch to the sibling branch (makes it live)");
                row(ui, "g / G", "first / last turn");
                if spatial {
                    ui.add_space(4.0);
                    ui.label(RichText::new("CAMERA").color(theme::DIM()).small().strong());
                    row(ui, "i / I", "zoom in / out");
                    row(ui, "z then h/l", "yaw");
                    row(ui, "z then k/j", "pitch");
                    row(ui, "o", "refit to the whole tree");
                }
                ui.add_space(4.0);
                ui.label(RichText::new("ACT").color(theme::DIM()).small().strong());
                row(ui, "a / Enter", "write (Insert) — Esc returns to motion");
                row(ui, "b", "branch off the focused turn");
                row(ui, "21j", "count prefix — repeat a motion (21 × j)");
                row(ui, "/", "find a message");
                row(ui, "space /", "search the tree");
                row(ui, "space \\", "fuzzy-search the tree");
                row(ui, ":", "command line");
                ui.add_space(4.0);
                ui.label(RichText::new("DEPTH SEARCH").color(theme::DIM()).small().strong());
                row(ui, ":6ca", "continue-all, fanned 6 rounds deep");
                row(ui, ":4fa 10 '…'", "fuzzy-all: depth 4 × width 10, templated");
                row(ui, "{lastmsg}", "in a :fa template → the leaf's text · {msgid} → its id");
                row(ui, ":fill [path]", "split a todo!()-skeleton into KV-shared sub-tasks");
                row(ui, ":assemble", "splice the filled bodies back into the skeleton");
                row(ui, ":best", "highlight the region that verified best");
                row(ui, ":stop", "cancel a running fan + clear the queue");
                ui.add_space(4.0);
                ui.label(RichText::new("Esc backs out of anything · ? closes this").color(theme::FAINT()).small());
                ui.add_space(2.0);
                if ui.small_button("close").clicked() { self.show_help = false; }
            });
    }

    /// Switch the central view and drop the modal into that view's natural mode: spatial starts in NORMAL
    /// (motions + indicator immediately live, the box releases focus); linear starts in INSERT (typing
    /// works). This is what makes the mode obvious the moment you enter the spatial graph.
    fn set_view(&mut self, spatial: bool, ui: &egui::Ui) {
        self.spatial_view = spatial;
        self.z_pending = false; self.leader_pending = false;
        if spatial {
            self.mode = crate::vimmode::Mode::Normal;
            ui.ctx().memory_mut(|m| m.surrender_focus(egui::Id::new("logscope_chat_composer")));
        } else {
            self.mode = crate::vimmode::Mode::Insert;
        }
    }

    /// AUTO-VERIFY: when a reply finishes producing Rust files, compile-check them automatically (offline
    /// `cargo check` in an isolated /tmp mirror) instead of waiting for a manual "Verify build" click — so a
    /// weak model that emits code that won't compile (bad `rand` call, i32-sorted-by-f64) is caught right
    /// away and the verdict is surfaced up top. No-op for non-Rust replies or while a check is already running.
    fn auto_verify_after_reply(&mut self, idx: usize) {
        if self.verify_job.as_ref().map(|j| j.result.is_none()).unwrap_or(false) { return; } // one already running
        let Some(m) = self.msgs.get(idx) else { return };
        if m.role != Role::Assistant { return; }
        let files: Vec<(String, String)> = crate::workspace::parse_artifacts(&m.text())
            .into_iter().filter(|a| a.path.ends_with(".rs")).map(|a| (a.path, a.content)).collect();
        if files.is_empty() { return; } // nothing compilable to judge
        let allow_net = self.verify_net;
        let (tx, rx) = std::sync::mpsc::channel();
        let root = std::env::temp_dir().join("logscope-verify-ui");
        std::thread::spawn(move || {
            let r = crate::sandbox::verify_files("generated", &files, &root, allow_net)
                .map(|r| (r.passed, r.output, r.sandboxed))
                .unwrap_or_else(|e| (false, format!("verify error: {e}"), false));
            let _ = tx.send(r);
        });
        self.verify_job = Some(VerifyJob { rx, started: Instant::now(), result: None });
    }

    /// Pump the live mic session once per frame: capture → whisper → finalized utterances / barge-in.
    /// Runs in both the linear and spatial views. Utterances route through `on_voice_utterance` (the single
    /// seam future voice features extend); a barge-in reaction while the model streams cancels generation.
    fn pump_voice(&mut self, infer: &Inference, doc: &crate::analysis::Doc, ui: &egui::Ui, streaming: bool) {
        // Arm barge-in only while the model generates; take the owned tick so the &mut borrow is released.
        let tick = match self.voice.as_mut() {
            Some(v) => { v.arm(streaming); v.poll() }
            None => return,
        };
        if streaming {
            if let Some(r) = &tick.reaction {
                if r.is_cancel() {
                    infer.cancel(); // spoke over the model
                    self.agent_grammar = None;
                if self.agent_run.take().is_some() || self.agent_fan.take().is_some() { self.agent_pending_submit = None; self.note = "agent run cancelled (barge-in)".into(); }
                }
            }
        }
        // A PANEL IS RUNNING: what you say competes for its interrupt
        // budget rather than becoming a chat turn.
        //
        // `voiceask` puts an operator interjection in the same currency
        // as a debater's question so `triage` can rank them against each
        // other. Without this the two could not be compared and whoever
        // spoke last would win, which is the opposite of a budget.
        //
        // Asks ACCUMULATE across frames on purpose: `triage` ranks a
        // batch, so a remark dropped the frame it was heard could never
        // compete with one made a moment later.
        if self.panel_mode && self.agent_fan.is_some() {
            let round = self.panel_round;
            self.pending_asks
                .extend(crate::voiceask::asks_from(&tick.utterances, round));
            // Budget of 1: an interruption is expensive and two at once
            // is not an interruption, it is a conversation the panel did
            // not agree to have.
            let chosen = logscope::panelvoice::triage(&self.pending_asks, round, 1);
            if let Some(a) = chosen.first() {
                self.note = format!("heard: {}", a.text);
            }
            // Cleared whether or not one was chosen. An ask that lost
            // this round has already been ranked against its
            // competitors; keeping it would let it win later on
            // staleness rather than on merit.
            self.pending_asks.clear();
            return;
        }
        for u in tick.utterances { self.on_voice_utterance(u, infer, doc, ui, streaming); }
    }

    /// The routing seam: what a finalized spoken utterance DOES. Parse it (voice_intent) and dispatch —
    /// graph commands ride the existing modal `run_command`; camera/send are handled here; anything else
    /// dictates into the composer. New voice behaviors are a match arm in voice_intent + (rarely) here.
    /// Freeze camera traversal where it is: stop the tour, exit freecam, and cancel any in-flight glide, but
    /// keep the current framing (spatial_zoomed) rather than snapping back to overview. This is what "hold
    /// here" / "wait a minute" / "stop" (mid-fly) do — the user wants to READ where they are.
    fn hold_camera(&mut self) {
        self.fly_auto = None;
        self.freecam = false;
        self.cam_pan_t0 = None;
        self.cam_pan_path.clear();
        self.cam_drift = None;       // stop directional travel too
        self.spatial_auto = false;   // don't re-frame to the centroid — hold this view
        self.spatial_zoomed = true;
    }

    fn on_voice_utterance(&mut self, u: crate::voicefan::Utterance, infer: &Inference, doc: &crate::analysis::Doc, ui: &egui::Ui, streaming: bool) {
        use crate::voice_intent::VoiceIntent;
        crate::clog!("voice", "acting on {:?}", u.text);
        match crate::voice_lang::parse_for_lang(&u.text) {
            VoiceIntent::Command(c) => {
                // Smart context: "stop"/"cancel" while a tour or freecam is running holds the CAMERA (what the
                // user means mid-fly) instead of cancelling generation. Otherwise Stop runs normally.
                if matches!(c, crate::vimmode::Command::Stop) && (self.fly_auto.is_some() || self.freecam) {
                    self.hold_camera();
                } else {
                    self.run_command(c, ui);
                }
            }
            VoiceIntent::HoldCamera => self.hold_camera(),
            VoiceIntent::Move(dir) => {
                // start continuous travel; also drop out of tour/freecam so nothing fights the drift
                self.fly_auto = None; self.freecam = false;
                self.cam_pan_t0 = None; self.cam_pan_path.clear();
                self.cam_drift = Some(dir);
                self.walk_ramp = 0.0;        // ease-in fresh each time a walk starts
                self.spatial_auto = false; self.spatial_zoomed = true;
            }
            VoiceIntent::Tour(on) => {
                self.fly_auto = if on { Some(1.0) } else { None };
                self.freecam = false;
                if !on { self.spatial_auto = true; self.spatial_zoomed = false; }
            }
            VoiceIntent::Freecam(on) => {
                self.freecam = on;
                if on { self.fly_auto = None; self.spatial_auto = false; self.spatial_zoomed = true; }
                else { self.spatial_auto = true; self.spatial_zoomed = false; }
            }
            VoiceIntent::CodeMode(on) => { self.code_voice = on; }
            VoiceIntent::Send => {
                if !streaming && !self.input.trim().is_empty() { self.send(infer, doc, ui); }
            }
            VoiceIntent::Dictate(text) => {
                // coder dictation: spoken symbols → punctuation ("open paren" → "("), unless prose mode
                let text = if self.code_voice { crate::voice_dict::dictate(text.trim()) } else { text.trim().to_string() };
                let text = text.trim();
                if text.is_empty() { return; }
                // A reaction spoken during a RUNNING agent (single run OR fan) forks/seeds parallel branches
                // without stopping it (5): a single run becomes a fan, a fan gains branches.
                if (self.agent_fan.is_some() || self.agent_run.is_some()) && self.seed_reaction_branches(text) { return; }
                if !self.input.is_empty() && !self.input.ends_with(char::is_whitespace) { self.input.push(' '); }
                self.input.push_str(text);
            }
        }
    }

}


/// Per-scene lighting profile: (sky bounce, ground bounce, ambient, exposure trim). Drives each space's mood —
/// the previous single fixed fill made everything read the same. Rooms inherit their LOGSCOPE_THEME profile.
/// The graph's own shortest path between two nodes, following parent links via their lowest common ancestor:
/// `from → … → LCA → … → to`. Used to glide the camera along the conversation's structure, not a straight line.
fn graph_path(g: &crate::convograph::Graph, from: usize, to: usize) -> Vec<usize> {
    if from == to { return vec![to]; }
    let mut up_from = vec![from];
    let mut x = from;
    while let Some(p) = g.nodes.get(x).and_then(|n| n.parent) { up_from.push(p); x = p; if up_from.len() > 4096 { break; } }
    let mut up_to = vec![to];
    let mut y = to;
    while !up_from.contains(&y) {
        match g.nodes.get(y).and_then(|n| n.parent) { Some(p) => { up_to.push(p); y = p; } None => break }
        if up_to.len() > 4096 { break; }
    }
    let lca = y;
    let mut path = Vec::new();
    for &nid in &up_from { path.push(nid); if nid == lca { break; } } // from → lca (nearest-first)
    if path.last() != Some(&lca) { path.push(lca); }                   // (to-side bottomed out at a root)
    for &nid in up_to.iter().rev() { if nid != lca { path.push(nid); } } // lca → to
    path
}

fn scene_light(name: &str) -> (egui::Color32, egui::Color32, f32, f32) {
    let c = |r, g, b| egui::Color32::from_rgb(r, g, b);
    match name {
        // Atlantis was left at full exposure while its bright-scene peers (Reading Room, Cloudscape,
        // Mansion Garden — see their comments above) all got trimmed for "blowing whites". Atlantis
        // ALSO gets an extra +12% on top from the aquatic exposure bump below, making its untrimmed
        // 0.98 the highest effective exposure of any theme once that stacks — confirmed by screenshot
        // comparison (a close-range llvmpipe render washed to near-solid cyan) and by ruling out bloom
        // and the sun-light colour first (changing either barely moved the result). Trimmed to land in
        // the same ballpark as Reading Room's already-corrected total (~1.3) once the aquatic bump and
        // the global grade multiplier are applied.
        "Atlantis"     => (c(0x6a, 0xc6, 0xd2), c(0x10, 0x2a, 0x36), 0.62, 0.70), // aqua, cool floor bounce
        "Canopy"       => (c(0x9a, 0xb2, 0x6e), c(0x22, 0x30, 0x1c), 0.56, 0.88), // dim mossy library green
        "Mistfall"     => (c(0xb2, 0xc6, 0xd4), c(0x20, 0x2a, 0x30), 0.62, 0.95), // diffuse misty grey-blue
        "Archive"      => (c(0xe8, 0xc2, 0x80), c(0x2a, 0x20, 0x14), 0.56, 0.95), // dim warm amber (wood)
        "Reading Room" => (c(0xe8, 0xe4, 0xd6), c(0x90, 0x86, 0x72), 0.60, 0.82), // airy paper — toned down (was blowing whites)
        "Folding@home" => (c(0x9a, 0xb0, 0xd0), c(0x16, 0x1c, 0x26), 0.70, 1.00), // cool clinical
        "Cloudscape"   => (c(0x4e, 0x5e, 0x82), c(0x20, 0x28, 0x3a), 0.50, 0.60), // MOONLIT night sky — cool, dim (was reading like daylight)
        "Forest"       => (c(0xd0, 0xe2, 0x8a), c(0x2e, 0x3c, 0x1e), 0.86, 1.06), // sunlit woodland green
        "Treehouse"    => (c(0xe2, 0xb0, 0x6e), c(0x1c, 0x26, 0x12), 0.58, 1.0),  // warm dusk canopy (fireflies pop)
        "Mansion Garden" => (c(0xd2, 0xc8, 0xb0), c(0x76, 0x6e, 0x58), 0.54, 0.70), // cream marble — toned further (still too bloomy)
        "Cat Cave"     => (c(0xd8, 0xb0, 0x7a), c(0x2a, 0x30, 0x2c), 0.70, 0.92), // amber lamplight bounce, cold wet floor
        "Forest Village" => (c(0xe6, 0xb8, 0x74), c(0x1c, 0x22, 0x2c), 0.60, 0.96), // lantern-warm sky bounce over wet blue stone
        "Lavarock Way" => (c(0xc8, 0xd8, 0xe8), c(0x2e, 0x40, 0x24), 0.46, 0.60), // bright afternoon — trimmed hard: llvmpipe blew the grass to neon at 0.86
        _              => (c(140, 168, 205),    c(96, 82, 66),       0.74, 1.00),
    }
}





/// One engine diagnostic per row: severity by colour, shortened with the full text on
/// hover, and a pill when the line folded onto itself.
///
/// Lifted out of the Engine page so a grouped section and a loose line draw identically.
/// Two copies of this would drift, and the difference would read as the grouping changing
/// what a line means.
fn draw_engine_lines(ui: &mut egui::Ui, entries: &[crate::diagfeed::Entry]) {
    // STAGGERED. A group that appears as one block reads as the pane redrawing; arriving in
    // order reads as lines being reported, which is what they are — the engine emits them one
    // at a time and the pane was the only thing that made them look simultaneous.
    //
    // Keyed on the group's first line so a group that is unchanged does not replay its
    // entrance on every repaint, which would be a pane that never settles.
    let key = entries.first().map(|e| e.text.clone()).unwrap_or_default();
    let id = ui.id().with(("engine-lines", &key));
    let born: f64 = ui.ctx().data_mut(|d| *d.get_temp_mut_or_insert_with(id, || ui.input(|i| i.time)));
    let elapsed = (ui.input(|i| i.time) - born) as f32;
    for (i, e) in entries.iter().enumerate() {
        // 40ms apart, 140ms each, and the whole run capped at 300ms: past that a stagger
        // stops being legible and becomes a wait for the last line.
        let t = uikit_egui::motion::stagger(elapsed, i, 0.04, 0.14, 0.30);
        if t < 1.0 {
            ui.ctx().request_repaint();
        }
        ui.set_opacity(t.max(0.02));
        let col = match e.kind {
            crate::diagfeed::Kind::Failure => Some(egui::Color32::from_rgb(235, 110, 100)),
            crate::diagfeed::Kind::Warning => Some(egui::Color32::from_rgb(230, 170, 80)),
            crate::diagfeed::Kind::Performance => None,
            crate::diagfeed::Kind::Context => Some(egui::Color32::from_gray(140)),
        };
        let line = crate::diagfeed::shorten(&e.text, 72);
        let rt = RichText::new(line).small();
        let rt = match col { Some(c) => rt.color(c), None => rt };
        ui.horizontal(|ui| {
            // Full text on hover: shortening keeps the panel readable, and the tail is
            // where the engine explains itself.
            ui.label(rt).on_hover_text(&e.text);
            if e.count > 1 {
                theme::event_pill(ui, e.kind.short_label(), e.count, e.kind == crate::diagfeed::Kind::Failure);
            }
        });
    }
}

// The tests live in `chat_tests.rs` — a CHILD module rather than a sibling, because they read
// this module's private items and only a child can. See that file's header for why the split.
#[cfg(test)]
#[path = "chat_tests.rs"]
mod chat_tests;
