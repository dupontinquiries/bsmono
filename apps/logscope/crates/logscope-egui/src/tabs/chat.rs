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

#[derive(Clone, Copy, PartialEq, Debug)]
enum Role { User, Assistant }
/// A turn. `variants` lets an assistant turn hold multiple completions (reroll / branch); `cur`
/// selects which is shown and which streaming appends to. User turns carry a single variant plus
/// inline-edit state.
struct Msg { role: Role, variants: Vec<String>, cur: usize, born: std::time::Instant, editing: bool, edit_buf: String, think_open: bool, parent: Option<usize> }
impl Msg {
    fn user(text: String) -> Self { Self { role: Role::User, variants: vec![text], cur: 0, born: Instant::now(), editing: false, edit_buf: String::new(), think_open: true, parent: None } }
    fn assistant() -> Self { Self { role: Role::Assistant, variants: vec![String::new()], cur: 0, born: Instant::now(), editing: false, edit_buf: String::new(), think_open: true, parent: None } }
    fn text(&self) -> &str { &self.variants[self.cur] }
    fn cur_mut(&mut self) -> &mut String { &mut self.variants[self.cur] }
}
/// A structural edit requested from a message's controls, applied after the render pass (so the
/// render can borrow `msgs` immutably-ish while the buttons just record intent).
enum MsgAction { Copy(usize), EditStart(usize), EditSave(usize), EditCancel(usize), Regenerate(usize), Delete(usize), PrevVar(usize), NextVar(usize) }
struct Stream { idx: usize, started: Instant, tokens: usize }
/// A background `cargo check` of the latest generated files (the sandbox oracle), run off-thread so
/// the UI stays responsive. `result` = (passed, compiler_output, sandboxed).
struct VerifyJob {
    rx: std::sync::mpsc::Receiver<(bool, String, bool)>,
    started: Instant,
    result: Option<(bool, String, bool)>,
}

// ── Persistence: checkpoints, rollback, crash recovery ───────────────────────────────────────
/// On-disk form of a turn (no UI/Instant state). Role 0 = user, 1 = assistant.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct MsgSave { role: u8, variants: Vec<String>, cur: usize }
/// A named, restorable snapshot of the whole conversation.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Checkpoint { label: String, at_unix: u64, msgs: Vec<MsgSave> }
/// What gets written to disk on every turn — the live conversation plus its checkpoints — so a
/// crash (e.g. an OOM mid-generation) loses nothing and the user can reopen and recover.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct ChatSave { msgs: Vec<MsgSave>, checkpoints: Vec<Checkpoint> }

fn msg_to_save(m: &Msg) -> MsgSave { MsgSave { role: if m.role == Role::User { 0 } else { 1 }, variants: m.variants.clone(), cur: m.cur } }
fn save_to_msg(s: &MsgSave) -> Msg {
    let variants = if s.variants.is_empty() { vec![String::new()] } else { s.variants.clone() };
    let cur = s.cur.min(variants.len() - 1);
    Msg { role: if s.role == 0 { Role::User } else { Role::Assistant }, variants, cur, born: Instant::now(), editing: false, edit_buf: String::new(), think_open: true, parent: None }
}
fn now_unix() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

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

/// Split a non-thinking span into prose + fenced code blocks (handles an unclosed fence).
fn segment_codeprose(text: &str, out: &mut Vec<Seg>) {
    let (mut prose, mut code, mut lang) = (String::new(), String::new(), String::new());
    let mut in_code = false;
    for line in text.split('\n') {
        if line.trim_start().starts_with("```") {
            if in_code {
                out.push(Seg::Code { lang: std::mem::take(&mut lang), body: code.trim_end_matches('\n').to_string() });
                code.clear(); in_code = false;
            } else {
                if !prose.trim().is_empty() { out.push(Seg::Prose(prose.trim_end().to_string())); }
                prose.clear();
                lang = line.trim_start().trim_start_matches('`').trim().to_string();
                in_code = true;
            }
            continue;
        }
        if in_code { code.push_str(line); code.push('\n'); }
        else { prose.push_str(line); prose.push('\n'); }
    }
    if in_code { out.push(Seg::Code { lang, body: code.trim_end_matches('\n').to_string() }); }
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
    let strong = ui.visuals().strong_text_color();
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    for r in parse_inline(line) {
        let font = FontId::new(size, if r.code { FontFamily::Monospace } else { FontFamily::Proportional });
        let color = if r.code || r.link { theme::CYAN() } else if r.bold { strong } else { base };
        let mut fmt = TextFormat { font_id: font, color, ..Default::default() };
        fmt.italics = r.italic;
        if r.link { fmt.underline = egui::Stroke::new(1.0, theme::CYAN()); }
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
            ui.label(RichText::new(format!("{body}{suffix}")).size(size).strong().color(theme::INK()));
            continue;
        }
        // bullet list item
        if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).or_else(|| trimmed.strip_prefix("+ ")) {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("•").color(theme::DIM()));
                ui.add_space(4.0);
                ui.label(md_line_job(ui, &format!("{rest}{suffix}"), base, theme::INK()));
            });
            continue;
        }
        if t.is_empty() { ui.add_space(4.0); continue; }
        ui.label(md_line_job(ui, &format!("{t}{suffix}"), base, theme::INK()));
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

/// Which subpage the right sidebar shows. The sidebar consolidates the per-conversation panels
/// (generated files, context/system settings, prompting modes) behind a button rail.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RightPage { Files, Graph, Context, Prompting }

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
struct Fan { kind: FanKind, rounds_left: usize }
const MAX_FAN_QUEUE: usize = 96;

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
    msgs: Vec<Msg>,
    input: String,
    context: String,   // system prompt — the "context" the model always sees
    threading: bool,   // thread prior turns into the prompt, or single-shot
    attach_log: bool,  // include a bounded summary of the current log in the system prompt
    fix_mode: bool,    // step-up submode: emit a concrete fix (file/diff) for the log/code
    show_context: bool, // right sidebar open/closed
    right_page: RightPage, // active sidebar subpage
    coord_mode: crate::coordinator::Mode, // prompting mode (barebones / coordinator / aggressive)
    verify_job: Option<VerifyJob>, // running sandbox build-check of the generated files
    verify_net: bool,              // allow the verify to fetch missing deps from crates.io (opt-in)
    show_files: bool,              // expand the generated-files drawer (toggled from the composer)
    walk: crate::walkcam::WalkCam, // prezi-style walkthrough camera over the graph
    walk_focus: Option<usize>,     // node the walkthrough is centered on
    cam_pan_path: Vec<usize>,      // node-id path (graph shortest path) the camera is gliding along
    cam_pan_t0: Option<std::time::Instant>, // start of the current glide; None = not panning
    scroll_chat_to: Option<usize>, // pending jump-scroll of the chat log to this message index
    orbit: crate::spatial::OrbitCam, // 3D camera for the spatial message view
    pearl_input: crate::pearl_input::PearlInput, // the composer rendered as a resonance-cell pearl
    pearl_on: bool,        // show the in-scene refractive pearl (toggle in the spatial toolbar)
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
    mode: crate::vimmode::Mode,    // modal control (Normal/Insert/Command/Search) — both views now
    cmdline: String,               // the `:` command buffer
    search_q: String,              // the `/` search buffer
    search_kind: crate::vimmode::SearchKind,
    z_pending: bool,               // a `z` rotation prefix is armed
    leader_pending: bool,          // a `space` leader is armed
    request_smaller_model: bool,   // :why asked to spawn the smallest local model for the diagnosis; ui() applies it
    show_help: bool,               // `?` keybinding overlay is open
    note: String,                  // transient status line (e.g. export path), cleared on next action
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
    preview: Option<usize>,                 // index of the generated file whose body is expanded
    auto_continue: bool,                    // decode a fresh reply continuously up to the reply ceiling
    hit_cap: bool,                          // last finished reply ran the full budget (offer manual Continue)
    checkpoints: Vec<Checkpoint>,           // restorable conversation snapshots (rollback)
    show_checkpoints: bool,                 // checkpoints panel open
    checked_restore: bool,                  // ran the startup autosave check
    recover: Option<usize>,                 // autosave found at startup with N msgs (offer recovery)
    vault: Option<crate::vault::Vault>,     // project-history store (content-addressed tree); None if unavailable
    head: Option<crate::vault::NodeId>,     // current node in the history tree
    show_history: bool,                     // history/tree panel open
    file_hist: Option<usize>,               // generated-file index whose version history is expanded
    template: TemplateKind,                 // chat-template family of the loaded model (set on load)
}
impl Default for Chat {
    fn default() -> Self {
        Self {
            msgs: Vec::new(), input: String::new(),
            context: BASE_SYSTEM.into(),
            threading: true, show_context: false, attach_log: true, fix_mode: false,
            right_page: RightPage::Prompting, coord_mode: crate::coordinator::Mode::Barebones,
            verify_job: None,
            verify_net: false,
            show_files: false,
            walk: crate::walkcam::WalkCam::new(13.0), walk_focus: None, scroll_chat_to: None,
            cam_pan_path: Vec::new(), cam_pan_t0: None,
            orbit: crate::spatial::OrbitCam::default(), spatial_view: false,
            pearl_input: Default::default(),
            pearl_on: false,
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
            mode: crate::vimmode::Mode::Insert, cmdline: String::new(), search_q: String::new(),
            search_kind: crate::vimmode::SearchKind::Tree, z_pending: false, leader_pending: false,
            request_smaller_model: false,
            show_help: false,
            note: String::new(),
            stream: None, last: None, ctx: Ctx::default(),
            backend: BackendKind::preferred(), sent_params: None, sent_backend: None, pending_cancel: false,
            ws_note: None, save_picker: Default::default(),
            attach_picker: Default::default(), save_dir: None, last_saved: None, preview: None,
            auto_continue: true, hit_cap: false,
            checkpoints: Vec::new(), show_checkpoints: false, checked_restore: false, recover: None,
            vault: autosave_path().and_then(|p| p.parent().map(|d| d.join("vault")))
                .and_then(|d| crate::vault::Vault::open(d).ok()),
            head: None, show_history: false, file_hist: None,
            template: TemplateKind::Llama3,
        }
    }
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
        match e {
            Evt::Token(t) => {
                let mut stop = false;
                let plain = self.backend != BackendKind::Wgpu; // template path stops on <|eot_id|>
                if let Some(s) = &mut self.stream {
                    s.tokens += 1;
                    if let Some(m) = self.msgs.get_mut(s.idx) {
                        m.cur_mut().push_str(&t);
                        if plain {
                            let cut = ["\nUser:", "\nuser:", "\nUSER:"].iter().filter_map(|mk| m.text().find(mk)).min();
                            if let Some(cut) = cut { m.cur_mut().truncate(cut); stop = true; }
                        }
                    }
                }
                if stop {
                    if let Some(s) = self.stream.take() { self.last = Some((s.tokens, s.started.elapsed().as_secs_f32())); }
                    self.pending_cancel = true;
                }
            }
            Evt::Done { tokens, secs } => {
                let idx = self.stream.as_ref().map(|s| s.idx);
                if let Some(i) = idx {
                    if let Some(m) = self.msgs.get_mut(i) {
                        if m.text().is_empty() { *m.cur_mut() = "[stopped]".into(); }
                    }
                }
                self.last = Some((tokens, secs));
                self.stream = None;
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
            }
            Evt::Error(e) => {
                if let Some(s) = &self.stream {
                    if let Some(m) = self.msgs.get_mut(s.idx) {
                        if m.text().is_empty() { *m.cur_mut() = format!("[error] {e}"); } else { let x = format!("\n[error] {e}"); m.cur_mut().push_str(&x); }
                    }
                }
                self.stream = None;
            }
            Evt::Loaded(Ok(info)) => { self.template = info.template; }
            Evt::Loaded(Err(_)) => {}
        }
    }

    fn params(&self) -> GenParams {
        // Auto-continue → decode a fresh reply continuously up to N chunks in one session; the slider
        // value stays the per-chunk / manual-continue size.
        let ceiling = if self.auto_continue { self.ctx.max_tokens * (1 + MAX_AUTO_CONTINUE) } else { self.ctx.max_tokens };
        GenParams { max_tokens: self.ctx.max_tokens as usize, reply_ceiling: ceiling as usize, n_ctx: self.ctx.n_ctx, n_batch: self.ctx.n_ctx,
            temperature: self.ctx.temperature, repeat_penalty: self.ctx.repeat_penalty, min_p: self.ctx.min_p, n_threads: 4, n_gpu_layers: -1 }
    }

    fn build_prompt(&self, doc: &crate::analysis::Doc) -> String { self.build_prompt_upto(self.msgs.len(), doc) }

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
        let leaves = self.leaf_ids();
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
        self.spatial_ui(ui);
    }

    /// Render the 3D mind-palace ONLY — no spatial chrome (Overview/Prune buttons, focused-node hint), filling
    /// the whole rect — for use as a full-window background layer behind a translucent UI (glass mode).
    /// App tells the chat whether it's hosted in glass mode (so the central canvas stays transparent and the
    /// full-window backdrop draws the scene instead — avoids a double scene render).
    pub fn set_glass(&mut self, g: bool) { self.glass_host = g; }

    pub fn scene_backdrop(&mut self, ui: &mut egui::Ui, frost_a: f32, frost_b: f32) {
        let prev = self.spatial_view;
        self.spatial_view = true;
        self.backdrop = true;
        self.frost_a = frost_a;
        self.frost_b = frost_b;
        self.spatial_ui(ui);
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
        self.msgs.push(Msg::assistant());
        let aid = self.msgs.len() - 1;
        self.msgs[aid].parent = Some(leaf);
        self.active_leaf = Some(aid);
        self.hit_cap = false;
        self.stream = Some(Stream { idx: aid, started: Instant::now(), tokens: 0 });
        self.last = None;
        infer.generate(prompt);
    }

    /// Build the prompt from history `msgs[..upto]` (so regenerate/edit can re-run from a point).
    fn build_prompt_upto(&self, upto: usize, doc: &crate::analysis::Doc) -> String {
        let sys = neutralize_markers(&self.full_system(doc));
        match self.backend {
            BackendKind::Wgpu => match self.template {
                TemplateKind::ChatML => self.build_prompt_chatml(&sys, upto),
                TemplateKind::Llama3 => self.build_prompt_llama3(&sys, upto),
                TemplateKind::Plain => self.build_prompt_plain(&sys, upto),
            },
            _ => self.build_prompt_plain(&sys, upto),
        }
    }

    /// Prompt to CONTINUE the last (partial) assistant reply: the same context the turn had, then
    /// the assistant turn left OPEN with the partial text and no closing marker, so the model picks
    /// up exactly where it was cut off rather than starting a new turn. Streaming appends straight
    /// into the same message. Uses the turns BEFORE the partial (chosen_msgs(.., last)) so the user
    /// question is still present even in single-shot mode.
    fn build_continue_prompt(&self, doc: &crate::analysis::Doc) -> String {
        let sys = neutralize_markers(&self.full_system(doc));
        let last = self.msgs.len().saturating_sub(1);
        let partial = neutralize_markers(&self.msgs.get(last).map(|m| m.text().to_string()).unwrap_or_default());
        let chatml = self.backend == BackendKind::Wgpu && self.template == TemplateKind::ChatML;
        let llama3 = self.backend == BackendKind::Wgpu && self.template == TemplateKind::Llama3;
        if chatml {
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
        self.stream = Some(Stream { idx: last, started: Instant::now(), tokens: 0 });
        self.last = None;
        infer.continue_stream(fallback);
    }

    // ── persistence / checkpoints / recovery ──
    /// Write the live conversation + checkpoints to disk (best-effort). Called on each completed
    /// turn and on checkpoint changes, so a crash mid-generation recovers everything.
    fn autosave(&self) {
        let Some(path) = autosave_path() else { return };
        let save = ChatSave { msgs: self.msgs.iter().map(msg_to_save).collect(), checkpoints: self.checkpoints.clone() };
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
        }
        self.recover = None;
    }

    /// The conversation as portable Markdown (You / Assistant sections).
    fn transcript(&self) -> String {
        let mut s = String::from("# logscope chat\n\n");
        for m in &self.msgs {
            s.push_str(if m.role == Role::User { "## You\n\n" } else { "## Assistant\n\n" });
            s.push_str(m.text());
            s.push_str("\n\n");
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
        if self.fix_mode { s.push_str("\n\n"); s.push_str(FIX_MODE); }
        s
    }

    fn log_block(&self, doc: &crate::analysis::Doc) -> Option<String> {
        if !self.attach_log || doc.lines.is_empty() { return None; }
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
        g
    }

    /// Most-recent turns that fit the budget — the in-context slice of the conversation graph, i.e. the
    /// SAME selection the Graph sidebar paints. convograph is the single authority; this is what the
    /// model is actually fed. When threading is off, only the latest message is kept (context leads).
    fn chosen_msgs(&self, per_msg_overhead: usize, sys: &str, upto: usize) -> Vec<&Msg> {
        let budget = self.ctx.n_ctx.saturating_sub(self.ctx.max_tokens) as usize;
        let max_msgs = if self.threading { self.ctx.history_turns * 2 + 1 } else { 1 };
        let mut g = self.convo_graph(upto);
        g.mark_window_ex(budget, est(sys) + 8, per_msg_overhead, max_msgs);
        g.context_ids().into_iter().map(|id| &self.msgs[id]).collect()
    }

    fn sys(&self) -> &str {
        let s = self.context.trim();
        if s.is_empty() { BASE_SYSTEM } else { s }
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
    fn build_prompt_chatml(&self, sys: &str, upto: usize) -> String {
        let mut s = format!("<|im_start|>system\n{}<|im_end|>\n", sys);
        for m in self.chosen_msgs(8, sys, upto) {
            let role = if m.role == Role::User { "user" } else { "assistant" };
            s.push_str(&format!("<|im_start|>{role}\n{}<|im_end|>\n", neutralize_markers(m.text())));
        }
        s.push_str("<|im_start|>assistant\n");
        s
    }

    fn send(&mut self, infer: &Inference, doc: &crate::analysis::Doc) {
        let qt = self.input.trim().to_string();
        if qt.is_empty() { return; }
        self.input.clear();
        // Branch point: a human interjection forks off the selected node (or the previous tip).
        let parent = self.branch_from.take().or(self.active_leaf).or(self.msgs.len().checked_sub(1));
        // Fire-and-queue: if the model is busy, don't stop it — enqueue this (branch) turn to run as
        // soon as the current reply finishes. This is what makes branching non-blocking.
        if self.stream.is_some() { self.queue.push_back((qt, parent)); return; }
        self.start_generation(qt, parent, infer, doc);
    }

    /// Actually begin a generation: checkpoint, append the user turn (forked at `parent`) + an empty
    /// assistant turn, build the prompt for the live branch, and stream. Shared by send + the queue.
    fn start_generation(&mut self, qt: String, parent: Option<usize>, infer: &Inference, doc: &crate::analysis::Doc) {
        if !self.msgs.is_empty() {
            let preview: String = qt.chars().take(40).collect();
            self.checkpoint(format!("before: {preview}"));
        }
        self.msgs.push(Msg::user(qt));
        let uid = self.msgs.len() - 1;
        self.msgs[uid].parent = parent;
        self.active_leaf = Some(uid); // path now ends at the new user turn for prompt-building
        self.hit_cap = false;
        let prompt = self.build_prompt(doc);
        self.last_prompt_tokens = est(&prompt); // for the live progress estimate
        self.msgs.push(Msg::assistant());
        let aid = self.msgs.len() - 1;
        self.msgs[aid].parent = Some(uid);
        self.active_leaf = Some(aid);
        self.stream = Some(Stream { idx: aid, started: Instant::now(), tokens: 0 });
        self.last = None;
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
                self.msgs.push(Msg::assistant());
                let idx = self.msgs.len() - 1;
                self.msgs[idx].parent = Some(i);
                self.active_leaf = Some(idx);
                self.pruned.retain(|&p| p < self.msgs.len());
                self.stream = Some(Stream { idx, started: Instant::now(), tokens: 0 });
                self.last = None;
                infer.generate(prompt);
            }
            MsgAction::Regenerate(i) => {
                if self.stream.is_some() || i >= self.msgs.len() || self.msgs[i].role != Role::Assistant { return; }
                self.msgs.truncate(i + 1);                    // drop anything after this reply
                let prompt = self.build_prompt_upto(i, doc);  // history up to the preceding user turn
                self.msgs[i].variants.push(String::new());    // new branch alongside the old reply
                self.msgs[i].cur = self.msgs[i].variants.len() - 1;
                self.active_leaf = Some(i);
                self.stream = Some(Stream { idx: i, started: Instant::now(), tokens: 0 });
                self.last = None;
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
        let key = (self.ctx.n_ctx, self.ctx.max_tokens, (self.ctx.temperature * 100.0) as i32, self.ctx.history_turns);
        if self.sent_params != Some(key) { infer.set_params(self.params()); self.sent_params = Some(key); }
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
        const CAP: usize = 16 * 1024;
        match std::fs::read(path) {
            Ok(bytes) => {
                let truncated = bytes.len() > CAP;
                let text = String::from_utf8_lossy(&bytes[..bytes.len().min(CAP)]).to_string();
                let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
                let lang = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if !self.input.is_empty() && !self.input.ends_with('\n') { self.input.push('\n'); }
                self.input.push_str(&format!("\n```{lang} {name}\n{text}\n```\n"));
                if truncated { self.input.push_str(&format!("(attached {} truncated to first {} KB)\n", name, CAP / 1024)); }
            }
            Err(e) => { self.ws_note = Some((format!("attach error: {e}"), Instant::now())); }
        }
    }

    /// The spatial (3D) message view — messages as nodes in space, drag to orbit, scroll to zoom,
    /// click a node to focus it (preview below). Replaces the linear chat in aggressive mode. Renders
    /// via 2D projection of `spatial::layout` through `self.orbit`; the linear chat is left untouched.
    fn spatial_ui(&mut self, ui: &mut egui::Ui) {
        let g = self.convo_graph(self.msgs.len());
        if g.nodes.is_empty() {
            if !self.backdrop {
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                ui.label(RichText::new("Spatial view").color(theme::DIM()).strong());
                ui.label(RichText::new("Nodes appear here as you converse — drag to orbit, scroll to zoom.").color(theme::FAINT()).small());
            });
            }
            return;
        }
        let parents: Vec<Option<usize>> = g.nodes.iter().map(|n| n.parent).collect();
        let n = g.nodes.len();
        // River layout: messages flow across the scene (advance by log-time, branches take their own lanes),
        // spread out and meandering rather than coiled into a radial helix. Cached — recomputed on tree change.
        if self.fold_pos.len() != n {
            let t0 = self.msgs.first().map(|m| m.born);
            let times: Vec<f64> = (0..n).map(|i| self.msgs.get(i)
                .and_then(|m| t0.map(|t0| m.born.saturating_duration_since(t0).as_secs_f64()))
                .unwrap_or(i as f64)).collect();
            self.fold_pos = crate::spatial::flow(&parents, &times);
        }
        let pos = {
            let mut p = self.fold_pos.clone();
            // raise the conversation off the floor to about the eye level of someone walking the hall, so the
            // fly camera (which rides the strand) moves through at head height rather than skimming the floor.
            if theme::active().name == "Atlantis" { for q in &mut p { q.y += 1.9; } }
            p
        };

        // section coloring (contiguous same-topic runs) + supersession (collapsible churn)
        let segs = g.segments();
        // PROCEDURAL THEMING: feed the conversation's shape to the environment generator so the hall is built
        // FROM the conversation — size from node count, colonnade from topic-segments, lanterns from branches,
        // and a content seed biases the tint. (Re-baked only when these change.)
        {
            let mut child_count: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();
            for nd in &g.nodes { if let Some(pid) = nd.parent { *child_count.entry(pid).or_insert(0) += 1; } }
            let branches = child_count.values().filter(|&&v| v > 1).count() as u32;
            let seed = (n as u32).wrapping_mul(2654435761)
                ^ (segs.len() as u32).wrapping_mul(40503)
                ^ branches.wrapping_mul(97)
                ^ g.nodes.first().map(|nd| nd.preview.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32))).unwrap_or(0);
            // graph half-extents per axis (x runs along the spine, z across it) so the hall is sized AND
            // shaped to CONTAIN the flow — a long spine makes an oval rotunda elongated along x.
            let (ext_x, ext_z) = self.fold_pos.iter().fold((0.0_f32, 0.0_f32), |(mx, mz), p| (mx.max(p.x.abs()), mz.max(p.z.abs())));
            // PER-BRANCH BEARINGS: for each fork (a node with >1 child) take the direction from the fork to the
            // centroid of its NON-main-line descendants — i.e. which way that branch actually peels off. These
            // route the wings (region_plan) to the branch's real direction instead of an even spread.
            let bear = {
                let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
                for nd in &g.nodes { if let Some(p) = nd.parent { if p < n && nd.id < n { children[p].push(nd.id); } } }
                let p_at = |i: usize| self.fold_pos.get(i).copied().unwrap_or(crate::spatial::Vec3::new(0.0, 0.0, 0.0));
                let (mut size, mut sx, mut sz, mut cnt) = (vec![1usize; n], vec![0f32; n], vec![0f32; n], vec![0usize; n]);
                for i in 0..n { let p = p_at(i); sx[i] = p.x; sz[i] = p.z; cnt[i] = 1; }
                for i in (0..n).rev() { let ch = children[i].clone(); for c in ch { size[i] += size[c]; sx[i] += sx[c]; sz[i] += sz[c]; cnt[i] += cnt[c]; } }
                let mut mainline = vec![false; n];
                if n > 0 { let mut cur = 0usize; loop { mainline[cur] = true; if children[cur].is_empty() { break; } let nx = *children[cur].iter().max_by_key(|&&c| size[c]).unwrap(); if nx == cur { break; } cur = nx; } }
                let mut bears: Vec<f32> = Vec::new();
                for f in 0..n {
                    if children[f].len() > 1 {
                        let (mut ax, mut az, mut ac) = (0f32, 0f32, 0usize);
                        for &c in &children[f] { if !mainline[c] { ax += sx[c]; az += sz[c]; ac += cnt[c]; } }
                        if ac > 0 { let fp = p_at(f); let (bx, bz) = (ax / ac as f32 - fp.x, az / ac as f32 - fp.z); if bx * bx + bz * bz > 1e-4 { bears.push(bz.atan2(bx)); } }
                    }
                }
                let mut bear = [0u16; 6];
                for (i, &a) in bears.iter().take(6).enumerate() {
                    let frac = a.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
                    bear[i] = 1 + (frac * 65534.0) as u16;
                }
                bear
            };
            let color_salience = chat_color_salience(self.msgs.iter().map(|m| m.text()));
            crate::scene3d::set_env_params(crate::scene3d::EnvParams {
                nodes: n as u32, branches, segments: segs.len() as u32, seed,
                ext_x: ext_x.ceil() as u32, ext_z: ext_z.ceil() as u32, bear,
                color_salience,
            });
        }
        let mut seg_of = vec![0usize; n];
        for (si, &(a, b)) in segs.iter().enumerate() { for id in a..=b.min(n.saturating_sub(1)) { seg_of[id] = si; } }
        let palette = [
            egui::Color32::from_rgb(0x6e, 0xc6, 0xff), egui::Color32::from_rgb(0x8b, 0xe0, 0x9b),
            egui::Color32::from_rgb(0xe6, 0xb4, 0x5c), egui::Color32::from_rgb(0xc9, 0x8f, 0xff),
            egui::Color32::from_rgb(0xff, 0x9b, 0x8a), egui::Color32::from_rgb(0x5c, 0xd6, 0xc4),
        ];
        let max_tok = g.nodes.iter().map(|nd| nd.tokens).max().unwrap_or(1).max(1) as f32;

        // controls: overview framing + human pruning to reclaim context room (hidden when used as a backdrop)
        if !self.backdrop {
        ui.horizontal(|ui| {
            // per-scene button SKIN: the default egui dark button reads as harsh dark contrast on the bright/warm
            // scenes. Give those a lightened, muted warm fill (reds/browns) with a low-contrast ink. (b_acc tints
            // the carved Overview button + its hover sheen.) Dark scenes keep the default skin.
            let nm = theme::active().name;
            let rgb = egui::Color32::from_rgb;
            let skin: Option<(egui::Color32, egui::Color32, egui::Color32, egui::Color32)> = match nm {
                // (accent, base fill, hover fill, ink)
                "Reading Room" => Some((rgb(0x9a, 0x52, 0x3c), rgb(0xe2, 0xd2, 0xc0), rgb(0xec, 0xdc, 0xc8), rgb(0x5e, 0x38, 0x28))), // muted terracotta on warm paper
                "Archive"      => Some((rgb(0xb0, 0x6e, 0x34), rgb(0x4a, 0x38, 0x24), rgb(0x5e, 0x48, 0x30), rgb(0xf0, 0xd8, 0xae))), // warm amber-brown
                "Canopy" | "Old Library" => Some((rgb(0x8a, 0x6e, 0x3a), rgb(0x3c, 0x32, 0x22), rgb(0x4e, 0x42, 0x2e), rgb(0xe6, 0xd8, 0xb6))), // mossy wood
                _ => None,
            };
            if let Some((_, bg, hov, ink)) = skin {
                let v = ui.visuals_mut();
                for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
                    w.bg_fill = bg; w.weak_bg_fill = bg; w.fg_stroke.color = ink;
                }
                v.widgets.hovered.bg_fill = hov; v.widgets.hovered.weak_bg_fill = hov;
                v.widgets.active.bg_fill = hov; v.widgets.active.weak_bg_fill = hov;
            }
            let overview = match skin { Some((acc, ..)) => theme::carved_button_tinted(ui, "⊹ Overview", acc), None => theme::carved_button(ui, "⊹ Overview") };
            if overview.on_hover_text("Recenter and fit the whole graph").clicked() {
                self.spatial_auto = true; self.spatial_zoomed = false; self.walk_focus = None; ui.ctx().request_repaint();
            }
            let sup = g.superseded();
            let prunable = sup.iter().filter(|id| !self.pruned.contains(id)).count();
            if ui.add_enabled(prunable > 0, egui::Button::new(RichText::new(format!("✂ Prune churn ({prunable})")).small()))
                .on_hover_text("Prune superseded turns — earlier iterations the conversation has moved past — to free context").clicked() {
                for id in sup { self.pruned.insert(id); }
                ui.ctx().request_repaint();
            }
            if ui.add_enabled(!self.pruned.is_empty(), egui::Button::new(RichText::new(format!("↩ Unprune ({})", self.pruned.len())).small()))
                .on_hover_text("Restore every pruned turn to context").clicked() {
                self.pruned.clear(); ui.ctx().request_repaint();
            }
            if !self.pruned.is_empty() {
                ui.label(RichText::new(format!("{} pruned · less context used", self.pruned.len())).color(theme::DIM()).small());
            }
        });
        ui.add_space(2.0);
        }

        let total = ui.available_size();
        // backdrop fills the whole rect; the interactive view reserves a thin strip for the focused-node hint
        let canvas_h = if self.backdrop { total.y.max(180.0) } else { (total.y - 30.0).max(180.0) };
        let (resp, p) = ui.allocate_painter(egui::vec2(total.x, canvas_h), egui::Sense::click_and_drag());
        let rect = resp.rect;
        // Opaque base under the backdrop so empty/sky regions never reveal undefined framebuffer memory (garbage
        // on some drivers). The GPU composite also forces opacity; this also covers the CPU software path.
        if self.backdrop {
            let b = theme::active().bg;
            let bg = egui::Color32::from_rgb((b.r() as f32 * 0.7) as u8, (b.g() as f32 * 0.7) as u8, (b.b() as f32 * 0.7) as u8);
            p.rect_filled(rect, 0.0, bg);
        }
        let (cx, cy) = (rect.center().x, rect.center().y);
        let scale = rect.height().min(rect.width()) * 0.9;

        // auto-frame: target the graph's centroid and fit the distance, so it sits centered (the prior
        // bug was a fixed target leaving the strand off-frame). The user orbiting/zooming/clicking opts
        // out; "Overview" opts back in. With auto on, the strand stays centered as it grows (the protein).
        let c = crate::spatial::centroid(&pos);
        if self.spatial_auto {
            self.orbit.target = c;
            if !self.spatial_zoomed { self.orbit.dist = crate::spatial::fit_distance(crate::spatial::bounding_radius(&pos, c)); }
        }
        if resp.dragged() {
            let d = resp.drag_delta();
            self.orbit.orbit(d.x * 0.01, -d.y * 0.01);
            ui.ctx().request_repaint();
        }
        if resp.hovered() {
            let sy = ui.input(|i| i.smooth_scroll_delta.y);
            if sy.abs() > 0.0 { self.orbit.zoom(1.0 - sy * 0.0015); self.spatial_zoomed = true; ui.ctx().request_repaint(); }
        }

        // SMOOTH GRAPH-PATH PAN: when a node is focused, glide the orbit target along the graph's OWN shortest
        // path (parent links via LCA) — Catmull-Rom through the node positions — instead of jumping straight there.
        if let Some(t0) = self.cam_pan_t0 {
            let np = self.cam_pan_path.len();
            if np >= 2 {
                use crate::spatial::Vec3 as V3;
                let pp: Vec<V3> = self.cam_pan_path.iter().map(|&id| pos[id.min(pos.len() - 1)]).collect();
                let dur = 0.30 + 0.10 * (np as f32 - 1.0);          // longer paths glide a little longer
                let raw = (t0.elapsed().as_secs_f32() / dur).clamp(0.0, 1.0);
                let e = raw * raw * (3.0 - 2.0 * raw);              // smoothstep ease
                let seg = e * (np - 1) as f32;
                let i0 = (seg.floor() as usize).min(np - 1);
                let i1 = (i0 + 1).min(np - 1);
                let fr = seg - i0 as f32;
                let g4 = |k: usize| pp[k.min(np - 1)];
                let cr = |p0: V3, p1: V3, p2: V3, p3: V3, u: f32| {
                    let (u2, u3) = (u * u, u * u * u);
                    let f = |a: f32, b: f32, c: f32, d: f32|
                        0.5 * ((2.0 * b) + (-a + c) * u + (2.0 * a - 5.0 * b + 4.0 * c - d) * u2 + (-a + 3.0 * b - 3.0 * c + d) * u3);
                    V3::new(f(p0.x, p1.x, p2.x, p3.x), f(p0.y, p1.y, p2.y, p3.y), f(p0.z, p1.z, p2.z, p3.z))
                };
                let here = cr(g4(i0.saturating_sub(1)), g4(i0), g4(i1), g4((i1 + 1).min(np - 1)), fr);
                self.orbit.target = here;
                self.spatial_auto = false;
                ui.ctx().request_repaint();
                if raw >= 1.0 { self.cam_pan_t0 = None; self.cam_pan_path.clear(); }
            } else { self.cam_pan_t0 = None; }
        }

        // Fly-through (headless / smooth camera move): LOGSCOPE_FLY=s (0..1) travels the camera target ALONG
        // the conversation path (id order) and FACES the direction of travel, sitting just inside the palace —
        // so we move THROUGH the mind palace. LOGSCOPE_FLY_AUTO=speed animates s from the clock for a
        // continuous glide. The forward look is derived from the path tangent (see OrbitCam::project).
        {
            let auto = std::env::var("LOGSCOPE_FLY_AUTO").ok().and_then(|v| v.trim().parse::<f32>().ok());
            let manual = std::env::var("LOGSCOPE_FLY").ok().and_then(|v| v.trim().parse::<f32>().ok());
            let s_opt = if let Some(spd) = auto {
                let tt = ui.input(|i| i.time) as f32;
                Some(((tt * spd * 0.05).fract() + 1.0).fract())
            } else { manual };
            if let (Some(s), true) = (s_opt, n > 1) {
                use crate::spatial::Vec3 as V3;
                let s = s.clamp(0.0, 0.999);
                let seg = s * (n - 1) as f32;
                let i0 = (seg.floor() as usize).min(n - 1);
                let i1 = (i0 + 1).min(n - 1);
                let fr = seg - i0 as f32;
                // Catmull-Rom through the 4 surrounding nodes → a smooth curve, not a polyline of kinks
                let cr = |p0: V3, p1: V3, p2: V3, p3: V3, u: f32| {
                    let u2 = u * u; let u3 = u2 * u;
                    let f = |a: f32, b: f32, c: f32, d: f32|
                        0.5 * ((2.0 * b) + (-a + c) * u + (2.0 * a - 5.0 * b + 4.0 * c - d) * u2 + (-a + 3.0 * b - 3.0 * c + d) * u3);
                    V3::new(f(p0.x, p1.x, p2.x, p3.x), f(p0.y, p1.y, p2.y, p3.y), f(p0.z, p1.z, p2.z, p3.z))
                };
                let pm = pos[i0.saturating_sub(1)]; let p1 = pos[i0]; let p2 = pos[i1]; let p3 = pos[(i1 + 1).min(n - 1)];
                let here = cr(pm, p1, p2, p3, fr);
                let look = cr(pm, p1, p2, p3, (fr + 0.16).min(1.0)); // a touch ahead on the same spline
                let (tx, ty, tz) = (look.x - here.x, look.y - here.y, look.z - here.z);
                let tl = (tx * tx + ty * ty + tz * tz).sqrt().max(1e-3);
                let (txn, tyn, tzn) = (tx / tl, ty / tl, tz / tl);
                let want_pitch = ((-tyn).asin().clamp(-1.2, 1.2)) + 0.12; // downward bias to see floor/pearls
                let want_yaw = txn.atan2(-tzn);
                let want_dist = std::env::var("LOGSCOPE_FLY_DIST").ok().and_then(|d| d.trim().parse().ok()).unwrap_or(5.2);
                // EASE the persistent orbit toward the desired pose each frame (smooth motion); converges within
                // the headless capture window and glides continuously in LOGSCOPE_FLY_AUTO mode.
                let k = 0.12;
                self.orbit.target.x += (here.x - self.orbit.target.x) * k;
                self.orbit.target.y += (here.y - self.orbit.target.y) * k;
                self.orbit.target.z += (here.z - self.orbit.target.z) * k;
                let mut dyaw = want_yaw - self.orbit.yaw;
                while dyaw > std::f32::consts::PI { dyaw -= std::f32::consts::TAU; }
                while dyaw < -std::f32::consts::PI { dyaw += std::f32::consts::TAU; }
                self.orbit.yaw += dyaw * k;
                self.orbit.pitch += (want_pitch - self.orbit.pitch) * k;
                self.orbit.dist += (want_dist - self.orbit.dist) * k;

                // CAMERA BOUNDING-SPHERE EVASION: keep the eye OUT of opaque solids (columns / central plinth)
                // and INSIDE the perimeter wall, using the colliders the env generator emitted from the same
                // EnvParams that built the hall. We shift the whole camera (target+eye) by the push so the look
                // direction is preserved — no more flying blind into a stone column or the plinth.
                let (cyf, syf) = (self.orbit.yaw.cos(), self.orbit.yaw.sin());
                let (cpf, spf) = (self.orbit.pitch.cos(), self.orbit.pitch.sin());
                let dd0 = self.orbit.dist;
                let eye = V3::new(self.orbit.target.x + dd0 * (-cpf * syf),
                                  self.orbit.target.y + dd0 * spf,
                                  self.orbit.target.z + dd0 * (cpf * cyf));
                let cam_r = 0.85f32; // camera collision volume (bounding-sphere radius)
                // openings (doors / hallway mouths) where the camera may cross the perimeter Bound
                let gates: Vec<(f32, f32)> = crate::scene3d::env_colliders().iter().filter_map(|c| match c {
                    crate::scene3d::Collider::Gate { ang, half } => Some((*ang, *half)), _ => None,
                }).collect();
                // floor/ceiling envelope from the solids, so the eye rides BETWEEN floor and dome (no diving
                // through the floor or punching the roof while it follows the strand).
                let (mut floor_y, mut ceil_y) = (f32::MAX, f32::MIN);
                for c in crate::scene3d::env_colliders() {
                    if let crate::scene3d::Collider::Pillar { y0, y1, .. } = c { floor_y = floor_y.min(y0); ceil_y = ceil_y.max(y1); }
                }
                let (mut px, mut py, mut pz) = (0.0f32, 0.0f32, 0.0f32);
                // a few relaxation passes so the eye settles in the GAP between solids (one pass under-resolves
                // when it's wedged between two columns), instead of tunnelling through a wall.
                for _ in 0..3 {
                    for col in crate::scene3d::env_colliders() {
                        match col {
                            crate::scene3d::Collider::Pillar { x, z, r, y0, y1 } => {
                                let ey = eye.y + py;
                                if ey > y0 - cam_r && ey < y1 + cam_r {
                                    let (dx, dz) = (eye.x + px - x, eye.z + pz - z);
                                    let dist = (dx * dx + dz * dz).sqrt();
                                    let rr = r + cam_r;
                                    if dist < rr {
                                        if dist > 1e-3 { let f = (rr - dist) / dist; px += dx * f; pz += dz * f; }
                                        else { px += rr; }
                                    }
                                }
                            }
                            crate::scene3d::Collider::Bound { r } => {
                                let (ex, ez) = (eye.x + px, eye.z + pz);
                                let dist = (ex * ex + ez * ez).sqrt();
                                let rr = r - cam_r;
                                if dist > rr && dist > 1e-3 {
                                    // let the camera slip OUT through a door / hallway mouth; otherwise hold it in
                                    let ang = ez.atan2(ex);
                                    let open = gates.iter().any(|&(ga, half)| { let d = (ang - ga).abs(); d.min(std::f32::consts::TAU - d) < half });
                                    if !open { let f = (dist - rr) / dist; px -= ex * f; pz -= ez * f; }
                                }
                            }
                            crate::scene3d::Collider::Gate { .. } => {} // handled above (pre-collected)
                        }
                    }
                }
                if floor_y.is_finite() {
                    let ey = eye.y + py;
                    let (lo, hi) = (floor_y + 1.2, ceil_y - 0.6);
                    if ey < lo { py += lo - ey; }
                    else if hi > lo && ey > hi { py += hi - ey; }
                }
                self.orbit.target.x += px; self.orbit.target.y += py; self.orbit.target.z += pz;

                self.spatial_auto = false; self.spatial_zoomed = true;
            }
        }

        // A gently drifting camera (slight parallax + rotation) for projection & rendering — additive on top
        // of the persistent orbit, so it breathes without ever running off into a full orbit. Used for the
        // dots, hit-testing, and the 3D meshes alike, so everything stays aligned.
        let view_cam = self.orbit.drifted(ui.input(|i| i.time) as f32);

        // project every node, then sort far→near (painter's algorithm)
        // LOGSCOPE_WORLD_ONLY=1 renders just the ENVIRONMENT (no graph overlay/bodies/caption) — for pixel-level
        // matching of the world against a reference, with the conversation data layer suppressed.
        let world_only = std::env::var("LOGSCOPE_WORLD_ONLY").is_ok();
        let mut proj: Vec<(usize, f32, f32, f32)> = Vec::with_capacity(n);
        for nd in &g.nodes {
            if let Some((sx, sy, depth)) = view_cam.project(pos[nd.id], cx, cy, scale) {
                proj.push((nd.id, sx, sy, depth));
            }
        }
        proj.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
        if world_only { proj.clear(); }  // suppress the 2D dot/cord/rune/label overlay
        let screen = |id: usize| proj.iter().find(|q| q.0 == id).map(|q| (q.1, q.2));

        // 3D mesh layer — each node a lit body (stones for Mistfall/Slate, leaf-puffs for Canopy, orbs
        // otherwise), strung on rippling strands (vine / water / air perturbation), shaded by a clock-driven
        // sun, drawn with the SAME camera as the dots. The dots/labels below sit on top for hit-testing.
        {
            use crate::mesh3d as m3;
            m3::set_tess_mul(theme::detail(theme::variant()).3); // detail variant scales all object tessellation
            let t = ui.input(|i| i.time) as f32;
            let name = theme::active().name;
            // PER-SCENE LIGHTING: Light::sun() only hands out a generic fill (fixed sky/ground/ambient) — so
            // every scene read the same. Give each its own sky/ground bounce + ambient + an exposure trim so the
            // spaces have distinct moods (aqua Atlantis · dappled Canopy · misty Mistfall · warm Archive ·
            // bright-airy Reading Room/Cloudscape · clinical Folding).
            let (lsky, lground, lamb, lexp) = scene_light(name);
            let mut light = m3::Light::sun(m3::sun_dir_3d(), theme::active().light);
            light.sky = lsky; light.ground = lground; light.ambient = lamb;
            let style = match name {
                "Canopy" => m3::NodeStyle::Leaf,
                "Mistfall" | "Slate" => m3::NodeStyle::Stone,
                "Atlantis" => m3::NodeStyle::Pearl,   // glossy prismarine pearls
                _ => m3::NodeStyle::Orb,
            };
            let edge_col = theme::LINE();
            let (strand_mat, node_mat) = match name { // strands = vine/water/stone; bodies = foliage/stone
                "Canopy" => (crate::texgen::mat::PAPER, crate::texgen::mat::MOSS), // air-field cords + leaf bodies
                "Mistfall" => (crate::texgen::mat::WATER, crate::texgen::mat::STONE),
                "Folding@home" => (crate::texgen::mat::PAPER, crate::texgen::mat::PAPER),
                "Atlantis" => (crate::texgen::mat::WATER, crate::texgen::mat::WATER), // glassy cords + glassy pearls
                "Treehouse" => (crate::texgen::mat::WOOD, crate::texgen::mat::WOOD),  // rope cords + carved wood balls
                _ => (crate::texgen::mat::STONE, crate::texgen::mat::STONE),
            };
            // Atlantis renders the graph as a beam array: pearls glow softly and the strands between them are
            // bright, near-straight LIGHT BEAMS (additive → caught by bloom). Mistfall keeps soft WATER cords;
            // other themes use plain edges.
            let beam = name == "Atlantis";
            let (cord_r, cord_amp, cord_glow, cord_seg) = if beam { (0.024, 0.05, 1.1, 7usize) }
                else if name == "Mistfall" { (0.05, 0.34, 0.0, 13) }
                else if name == "Canopy" { (0.02, 0.5, 0.30, 14) } // a vibrating AIR filament that holds the leaves
                else { (0.045, 0.18, 0.0, 7) };
            let pearl_glow = if name == "Atlantis" { 0.14 } else { 0.0 };
            // Mistfall: the floating stones carry a CARVED BLUE RUNE glow — one colour unified across the whole
            // graph (so the conversation reads as a single spell), shifted slightly by the env variant.
            let rune = name == "Mistfall";
            // Treehouse: the graph reads as carved WOOD BALLS hung by rope from the canopy overhead — each node
            // is a wooden orb with a rope rising above it (the "inscription" stays as the node's label).
            let treehouse = name == "Treehouse";
            let rune_col = {
                let h = (theme::variant() as u32).wrapping_mul(0x9E3779B9) >> 25; // 0..127 from the variant
                egui::Color32::from_rgb(0x3c + (h % 24) as u8, 0x96, 0xd8 + (h % 20) as u8)
            };
            let cord_col = if beam { egui::Color32::from_rgb(0xb6, 0xee, 0xff) } else if name == "Canopy" { egui::Color32::from_rgb(0xd8, 0xe4, 0xe8) } else { edge_col };
            let mut graph: Vec<m3::Mesh3> = Vec::with_capacity(n * 2);
            for nd in &g.nodes {
                if let Some(pid) = nd.parent {
                    let mut st = m3::with_mat(m3::strand(pos[pid], pos[nd.id], cord_r, 6, cord_seg, t, cord_amp, cord_col), strand_mat);
                    st.emissive = cord_glow;
                    if name == "Canopy" { m3::set_alpha(&mut st, 85); } // faint, see-through air
                    graph.push(st);
                }
            }
            for nd in &g.nodes {
                let active = self.active_leaf == Some(nd.id);
                let wood = [egui::Color32::from_rgb(0x8c, 0x66, 0x3c), egui::Color32::from_rgb(0x74, 0x52, 0x2e), egui::Color32::from_rgb(0x9c, 0x74, 0x46), egui::Color32::from_rgb(0x6a, 0x4a, 0x2a)];
                let col = if active { theme::accent() } else if rune { rune_col } else if treehouse { wood[nd.id % 4] } else { palette[seg_of[nd.id] % palette.len()] };
                let mut nm = if name == "Old Library" {
                    // a turn's FORM follows its content: short snippet → page, code/file/long → scroll (the
                    // sheet unrolls further the longer it is), otherwise a closed book. Code/file is sniffed
                    // from the preview (fences, braces, calls, a path with an extension).
                    let p = &nd.preview;
                    let codeish = p.contains('`') || p.contains('{') || p.contains(';') || p.contains("fn ")
                        || p.contains("def ") || p.contains("()") || p.contains("::") || (p.contains('/') && p.contains('.'));
                    let (kind, unroll) = if codeish || nd.tokens > 180 {
                        (2u32, (nd.tokens as f32 / 420.0).clamp(0.18, 1.0))
                    } else if nd.tokens < 42 { (0u32, 0.0) } else { (1u32, 0.0) };
                    m3::with_mat(m3::library_node(kind, pos[nd.id], 0.26, unroll, nd.id as u32, col), crate::texgen::mat::PAPER)
                } else if name == "Atlantis" {
                    // smooth nacreous PEARLS (the cracked-gem node read as a foreign asset / wrong texture);
                    // node_mat = WATER → the nacre/thin-film shading gives the pearly sheen.
                    m3::with_mat(m3::uv_sphere(pos[nd.id], 0.27, 22, 14, col), node_mat)
                } else {
                    // Slate reads too uniform → vary the stone SIZE by the turn's weight (on top of the
                    // per-seed shape variation rock() already gives); other scenes keep a steady node size.
                    let nr = if name == "Slate" { 0.17 + 0.20 * (nd.tokens as f32 / 150.0).clamp(0.0, 1.4) } else { 0.26 };
                    m3::with_mat(m3::node_mesh(style, pos[nd.id], nr, nd.id as u32, col), node_mat)
                };
                if name == "Atlantis" {
                    // pearls drift SLOWLY on the currents instead of sitting rigid on the beams (the offset is
                    // applied to the packed verts each frame, so they bob gently as the UI repaints).
                    let ph = nd.id as f32 * 1.7;
                    nm.offset = crate::spatial::Vec3::new(0.04 * (t * 0.30 + ph).sin(), 0.09 * (t * 0.24 + ph * 1.3).sin(), 0.04 * (t * 0.28 + ph * 0.7).cos());
                }
                nm.emissive = if rune { 0.40 + 0.10 * (t * 1.3 + nd.id as f32 * 0.5).sin() } else { pearl_glow }; // runes pulse faintly
                if active { nm.emissive = 0.9 + 0.35 * (t * 1.6).sin(); }
                graph.push(nm);
                // Treehouse: a rope rising from each wood ball to the canopy overhead — the balls HANG.
                if treehouse {
                    let p = pos[nd.id];
                    let top = crate::spatial::Vec3::new(p.x, p.y + 2.1, p.z);
                    let mut rope = m3::with_mat(m3::tube(top, p, 0.022, 5, egui::Color32::from_rgb(0x46, 0x34, 0x20)), crate::texgen::mat::WOOD);
                    if active { rope.emissive = 0.2; }
                    graph.push(rope);
                }
            }

            // pearl-reader: starry speckle cells inside NEAR pearls (LOD by projected depth), twinkling and
            // brightening as the camera approaches (resonance) — the living point-cloud pearl, in 3D.
            if name == "Atlantis" {
                let (mut mind, mut maxd) = (f32::MAX, f32::MIN);
                for q in &proj { mind = mind.min(q.3); maxd = maxd.max(q.3); }
                let span = (maxd - mind).max(1e-3);
                for q in &proj {
                    let near = ((maxd - q.3) / span).clamp(0.0, 1.0); // 1 = nearest the camera
                    if near < 0.4 { continue; }
                    let ncells = (near * near * 34.0) as usize;
                    if ncells < 5 { continue; }
                    let active = self.active_leaf == Some(q.0);
                    let ccol = if active { theme::accent() } else { theme::CYAN() };
                    for cell in m3::speckle_cells(pos[q.0], 0.26, q.0 as u32, t, ccol, ncells, near) {
                        graph.push(cell);
                    }
                }

                // text-on-approach: the NEAREST pearls resolve their turn label into glowing cells (resonance) —
                // the speckle is the body, the legible text condenses out of it as the camera draws close.
                let (vy, vp) = (view_cam.yaw, view_cam.pitch);
                let (cyaw, syaw) = (vy.cos(), vy.sin());
                let (cp, sp) = (vp.cos(), vp.sin());
                let right = crate::spatial::Vec3::new(cyaw, 0.0, syaw);
                let up = crate::spatial::Vec3::new(syaw * sp, cp, -cyaw * sp);
                let toward_cam = crate::spatial::Vec3::new(-(cp * syaw), sp, cp * cyaw); // -viewDir → toward eye
                let scroll = (t * 0.12).sin() * 0.06; // gentle teleprompter drift
                for q in &proj {
                    let near = ((maxd - q.3) / span).clamp(0.0, 1.0);
                    let mut lit = ((near - 0.80) / 0.18).clamp(0.0, 1.0); // resolves only when very close
                    if lit < 0.05 { continue; }
                    // occlusion dimming: if a NEARER pearl overlaps this one on screen, fade this label back so
                    // the front one reads cleanly (declutters stacked labels along the strand).
                    let mut occ = 0.0f32;
                    for r2 in &proj {
                        if r2.3 < q.3 - 0.1 {
                            let (dx, dy) = (r2.1 - q.1, r2.2 - q.2);
                            if dx * dx + dy * dy < 78.0 * 78.0 { occ += 1.0; }
                        }
                    }
                    lit /= 1.0 + 1.5 * occ;
                    if lit < 0.05 { continue; }
                    if let Some(nd) = g.nodes.iter().find(|nd| nd.id == q.0) {
                        let label: String = nd.preview.chars().take(16).collect();
                        if label.trim().is_empty() { continue; }
                        for cell in m3::label_cells(pos[q.0], right, up, toward_cam, label.trim(),
                                                    0.034, t, theme::CYAN(), lit, scroll) {
                            graph.push(cell);
                        }
                    }
                }
            }

            // Reflection — the water (or polished floor) holds a faded image of the conversation. Each node
            // and strand is mirrored onto the surface plane, dimmer where the real node rides higher (a
            // glancing reflection) and dimmer with age, so the past softens into the surface as "memory".
            // (A blended ghost on the surface; true sub-surface refraction would want a translucent-water
            // pass — a follow-up.)
            let (refl_amt, plane_y) = match name {
                "Mistfall" => (0.62_f32, -3.55_f32), // water — strong
                "Reading Room" => (0.30, -2.95),     // polished limestone — subtle
                "Canopy" | "Archive" => (0.20, -3.15),
                "Folding@home" => (0.0, 0.0),        // no reflective plane near the helix
                _ => (0.26, -2.95),
            };
            if refl_amt > 0.0 {
                let bg = theme::active().bg;
                let nmax = n.max(1) as f32;
                let fade = |p: crate::spatial::Vec3, id: usize, base: egui::Color32, peak: f32| {
                    let height_fade = 0.3 + 0.7 * (1.0 - ((p.y - plane_y) / 6.0).clamp(0.0, 1.0));
                    let age_fade = 0.5 + 0.5 * (id as f32 / nmax); // older ids fainter
                    let a = (peak * refl_amt * height_fade * age_fade) as u8;
                    let d = theme::lerp_col(base, bg, 0.5);
                    egui::Color32::from_rgba_unmultiplied(d.r(), d.g(), d.b(), a)
                };
                for nd in &g.nodes {
                    if let Some(pid) = nd.parent {
                        let a = crate::spatial::Vec3::new(pos[pid].x, plane_y, pos[pid].z);
                        let b = crate::spatial::Vec3::new(pos[nd.id].x, plane_y, pos[nd.id].z);
                        let mut st = m3::with_mat(m3::strand(a, b, 0.03, 4, 5, t, 0.10, edge_col), strand_mat);
                        if name == "Atlantis" { st.color = theme::lerp_col(cord_col, bg, 0.3); st.emissive = 0.32; }
                        else { st.color = fade(pos[nd.id], nd.id, edge_col, 70.0); }
                        graph.push(st);
                    }
                }
                for nd in &g.nodes {
                    let p = pos[nd.id];
                    let gp = crate::spatial::Vec3::new(p.x, plane_y, p.z);
                    let base = if self.active_leaf == Some(nd.id) { theme::CYAN() } else { palette[seg_of[nd.id] % palette.len()] };
                    let mut ghost = m3::with_mat(m3::node_mesh(style, gp, 0.17, nd.id as u32, base), node_mat);
                    if name == "Atlantis" {
                        // hollywood mirror: a softly glowing reflection in dark water, not an opaque dark blob
                        ghost.color = theme::lerp_col(base, bg, 0.35);
                        ghost.emissive = 0.40 * (0.5 + 0.5 * (nd.id as f32 / nmax)); // recent reflections brighter
                    } else {
                        ghost.color = fade(p, nd.id, base, 135.0);
                    }
                    graph.push(ghost);
                }
            }

            // PHYSICAL UI PANEL — theme-aware: each scene casts the message entry in its own idiom (Atlantis
            // pearl/nacre · Canopy backlit leaf-glow · Mistfall carved blue rune · Archive/Reading ink-on-paper).
            if !self.backdrop {
                use crate::spatial::Vec3 as V3;
                // (panel rgb, material, emissive, alpha, text rgb, text-glow multiplier)
                let (pc, pmat, pemis, palpha, tc, tglow): ([u8; 3], f32, f32, u8, [u8; 3], f32) = match name {
                    "Atlantis"     => ([0xe6, 0xec, 0xf2], crate::texgen::mat::WATER, 0.28, 118, [0x9c, 0xee, 0xff], 1.25),
                    "Canopy"       => ([0x86, 0xb8, 0x52], crate::texgen::mat::MOSS,  0.14, 122, [0xe8, 0xf6, 0x9a], 1.15), // backlit leaf glow
                    "Mistfall"     => ([0x46, 0x60, 0x78], crate::texgen::mat::STONE, 0.08, 158, [0x66, 0xc6, 0xff], 1.0),  // carved blue rune
                    "Archive"      => ([0xe6, 0xd8, 0xbc], crate::texgen::mat::PAPER, 0.04, 188, [0x4a, 0x32, 0x1c], 0.45), // ink on parchment
                    "Reading Room" => ([0xf0, 0xe8, 0xd6], crate::texgen::mat::PAPER, 0.04, 190, [0x2e, 0x24, 0x18], 0.42), // typewriter ink
                    _               => ([0xc8, 0xd2, 0xe0], crate::texgen::mat::STONE, 0.12, 134, [0x9c, 0xc8, 0xff], 1.0),
                };
                let (cyw, syw) = (view_cam.yaw.cos(), view_cam.yaw.sin());
                let (cpt, spt) = (view_cam.pitch.cos(), view_cam.pitch.sin());
                let right = V3::new(cyw, 0.0, syw);
                let up = V3::new(syw * spt, cpt, -cyw * spt);
                let toward = V3::new(-(cpt * syw), spt, cpt * cyw);
                let fwd = V3::new(cpt * syw, -spt, -cpt * cyw);
                let tg = view_cam.target;
                let eye = V3::new(tg.x + toward.x * view_cam.dist, tg.y + toward.y * view_cam.dist, tg.z + toward.z * view_cam.dist);
                let (d, drop) = (4.4_f32, -1.45_f32);
                let cen = V3::new(eye.x + fwd.x * d + up.x * drop, eye.y + fwd.y * d + up.y * drop, eye.z + fwd.z * d + up.z * drop);
                // bezel frame just BEHIND the plaque (darker shade of the panel, a touch more opaque) so the
                // message entry reads as a physical tablet/plaque rather than a floating quad.
                let bez = if name == "Atlantis" { egui::Color32::from_rgb(0x1c, 0x5c, 0x6a) } else { egui::Color32::from_rgb((pc[0] as f32 * 0.5) as u8, (pc[1] as f32 * 0.5) as u8, (pc[2] as f32 * 0.5) as u8) };
                let fcen = V3::new(cen.x + fwd.x * 0.05, cen.y + fwd.y * 0.05, cen.z + fwd.z * 0.05);
                let mut frame = m3::panel(fcen, right, up, 4.3, 1.06, bez);
                frame.mat = pmat; frame.emissive = if name == "Atlantis" { 0.34 } else { pemis * 0.4 }; m3::set_alpha(&mut frame, (palpha as u16 + 50).min(255) as u8);
                graph.push(frame);
                // a thin glowing accent strip down the left edge, in the scene's text colour — a small UI cue.
                let acen = V3::new(cen.x - right.x * 1.92, cen.y - right.y * 1.92, cen.z - right.z * 1.92);
                let mut accent = m3::panel(acen, right, up, 0.08, 0.66, egui::Color32::from_rgb(tc[0], tc[1], tc[2]));
                accent.mat = pmat; accent.emissive = 0.45 + pemis; m3::set_alpha(&mut accent, 224);
                graph.push(accent);
                let mut panel = m3::panel(cen, right, up, 4.0, 0.82, egui::Color32::from_rgb(pc[0], pc[1], pc[2]));
                panel.mat = pmat; panel.emissive = pemis;
                m3::set_alpha(&mut panel, palpha);
                graph.push(panel);
                let typing = !self.input.trim().is_empty();
                let shown: String = if typing {
                    let nch = self.input.chars().count();
                    self.input.chars().skip(nch.saturating_sub(20)).collect()
                } else { "speak into the pearl".chars().take(20).collect() };
                let lit = (if typing { 0.95_f32 } else { 0.34 }) * tglow;
                let tcol = egui::Color32::from_rgb(tc[0], tc[1], tc[2]);
                // LEFT-ALIGN from a fixed left edge so every glyph sits in a STABLE pixel column as the prompt
                // grows (label_cells centres on its origin, which would slide the text around as you type). Font
                // is ~12% smaller than before so longer prompts fit. This is what sells "typing into the panel".
                let tscale = 0.030_f32;
                let nshown = shown.trim().chars().count().min(32) as f32;
                let tw = nshown * 6.0 * tscale;
                let lx = 1.86_f32; // half the text field width (panel half is 2.0)
                let bx = V3::new(cen.x - right.x * lx, cen.y - right.y * lx, cen.z - right.z * lx); // fixed left edge
                let tcen = V3::new(bx.x + right.x * (tw * 0.5) + up.x * 0.06, bx.y + right.y * (tw * 0.5) + up.y * 0.06, bx.z + right.z * (tw * 0.5) + up.z * 0.06);
                for cell in m3::label_cells(tcen, right, up, toward, shown.trim(), tscale, t, tcol, lit, 0.0) {
                    graph.push(cell);
                }
                // blinking caret at the end of the entry (the input "cursor"), so the selection point is visible.
                if typing && (t * 2.0).sin() > 0.0 {
                    let cx = bx.x + right.x * (tw + tscale * 2.0);
                    let cyv = bx.y + right.y * (tw + tscale * 2.0) + up.y * 0.06;
                    let cz = bx.z + right.z * (tw + tscale * 2.0) + up.z * 0.06;
                    for ky in 0..7 {
                        let py = (3.0 - ky as f32) * tscale;
                        let mut cm = m3::uv_sphere(V3::new(cx + up.x * py, cyv + up.y * py - up.y * 0.0, cz + up.z * py), tscale * 0.5, 4, 3, tcol);
                        cm.emissive = 1.4 * tglow; graph.push(cm);
                    }
                }
            }

            // CPU path (default): software-shaded triangles into egui's painter.
            #[cfg(not(feature = "gpu-scene"))]
            {
                crate::scene3d::render_env(&p, &view_cam, cx, cy, scale, name, &light, t);
                if !world_only { p.add(m3::render(&graph, &view_cam, cx, cy, scale, &light)); }
            }
            // GPU path (`--features gpu-scene`): one depth-sorted GLSL pass (env behind graph) via egui_glow,
            // pixel-aligned with the dots/runes because the camera matrix reproduces OrbitCam::project.
            #[cfg(feature = "gpu-scene")]
            {
                let cam = crate::gpuscene::Camera::from_orbit(&view_cam, rect.width(), rect.height(), scale, 0.1, 400.0);
                let mut verts = Vec::new();
                crate::scene3d::with_env(name, t, |env| verts.extend(crate::gpuscene::pack_sorted(env, &cam)));
                if !world_only { verts.extend(crate::gpuscene::pack_sorted(&graph, &cam)); } // graph after env → drawn in front
                let sun = m3::sun_dir_3d();
                let c3 = |c: egui::Color32| [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0];
                let sun_uv = cam.dir_uv([sun.x, sun.y, sun.z]).unwrap_or([0.5, 1.25]); // above-screen if behind → faint
                let (gexp, gfogm, gbloom, gthr, gambm, gwarm) = theme::grade(theme::LOCKED_GRADE);
                let (_texn, dtexscale, dbump, _tess) = theme::detail(theme::variant()); // detail sweep → finer tiling + relief
                let fog = match name { // base fog per theme, then scaled by the locked grade
                    "Mistfall" => 0.044,  // still a misty veil, but you can see across the room
                    "Treehouse" => 0.022, // dusk haze blots distant trees (LOD by fog, not walls)
                    "Folding@home" => 0.014,
                    "Atlantis" => 0.009, // AIR interior reads clear; the water surfaces carry their own depth murk
                    _ => 0.011,           // Archive/Slate/Old Library/Reading Room: light haze only (was 0.017 → grey-out)
                } * gfogm * std::env::var("LOGSCOPE_FOGM").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0);
                // warm/cool shift the sun for the grade
                let warm_target = if gwarm >= 0.0 { egui::Color32::from_rgb(255, 224, 170) } else { egui::Color32::from_rgb(180, 205, 255) };
                let sun_graded = theme::lerp_col(light.color, warm_target, gwarm.abs());
                // TIME-OF-DAY sync: the local clock drives sun strength, warmth and fill on top of the grade —
                // warm low sun at dawn/dusk, bright neutral midday, dim cool at night.
                let (tod_strength, tod_warm, tod_amb) = m3::sun_clock();
                let tod_target = if tod_warm >= 0.0 { egui::Color32::from_rgb(255, 210, 150) } else { egui::Color32::from_rgb(150, 180, 255) };
                let sun_tod = theme::lerp_col(sun_graded, tod_target, tod_warm.abs() * 0.8);
                let caustic = match name { "Atlantis" => 0.5, "Mistfall" => 0.22, _ => 0.0 };
                let aquatic = name == "Atlantis";
                let sun_uv = if aquatic { [0.5, 0.10] } else { sun_uv }; // shafts pour down from the oculus overhead
                let (gr_amt, gr_w, gr_dec) = if aquatic { (0.9_f32, 0.52_f32, 0.95_f32) } else { (0.6, 0.45, 0.92) };
                // COMPOSER PEARL — env-driven for now (headless preview of the refractive pearl in-scene); the
                // interactive "cast your next pearl into the scene" flow will drive these live.
                let pearl = std::env::var("LOGSCOPE_PEARL").ok().and_then(|v| v.trim().parse::<f32>().ok())
                    .unwrap_or(if self.pearl_on { 1.0 } else { 0.0 });
                let pearl_variant = std::env::var("LOGSCOPE_PEARL_VARIANT").ok().and_then(|v| v.trim().parse::<f32>().ok())
                    .unwrap_or(self.pearl_variant as f32);
                let pearl_pos = std::env::var("LOGSCOPE_PEARL_POS").ok()
                    .and_then(|v| { let q: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect(); if q.len() == 2 { Some([q[0], q[1]]) } else { None } })
                    .unwrap_or([0.5, 0.40]);
                let pearl_r = std::env::var("LOGSCOPE_PEARL_R").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.26);
                // UNDERWATER: warps the view when the eye is in OPEN WATER. Procedural gen means "outside the
                // main Bound" is no longer the test — hallways + wings (at the Gate bearings) extend past the
                // Bound but are AIR. So: ramp on past the perimeter ONLY where the eye is NOT within a Gate
                // corridor (the openings the fly travels out through). Inside the room or any hallway/wing → dry.
                let underwater = if aquatic {
                    let e = cam.eye();
                    let cols = crate::scene3d::env_colliders();
                    let rp = cols.iter().find_map(|c| match c { crate::scene3d::Collider::Bound { r } => Some(*r), _ => None }).unwrap_or(f32::MAX);
                    let d = (e[0] * e[0] + e[2] * e[2]).sqrt();
                    if d <= rp - 1.0 {
                        0.0 // inside the main rotunda — always air
                    } else {
                        let theta = e[2].atan2(e[0]);
                        let ang_d = |a: f32, b: f32| { let mut x = (a - b).abs() % std::f32::consts::TAU; if x > std::f32::consts::PI { x = std::f32::consts::TAU - x; } x };
                        // a Gate corridor (hallway/wing) is air even past the Bound — a generous wedge covers the
                        // ellipse z-compression that skews the apparent bearing.
                        let in_corridor = cols.iter().any(|c| match c {
                            crate::scene3d::Collider::Gate { ang, half } => ang_d(theta, *ang) < half + 0.35,
                            _ => false,
                        });
                        if in_corridor { 0.0 } else { ((d - (rp - 1.0)) / 3.5).clamp(0.0, 1.0) }
                    }
                } else { 0.0 };
                let u = crate::glow_scene::Uniforms {
                    viewproj: cam.view_proj(), eye: cam.eye(),
                    sun_dir: [sun.x, sun.y, sun.z], sun_col: c3(sun_tod),
                    sky: c3(light.sky), ground: c3(light.ground), ambient: light.ambient * gambm * tod_amb,
                    fog_col: c3(theme::active().bg), fog, sun_uv, time: t,
                    exposure: gexp * lexp * (if aquatic { 1.12 } else { 1.0 }) * (0.55 + 0.48 * tod_strength)
                        * std::env::var("LOGSCOPE_EXP").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0),
                    saturation: std::env::var("LOGSCOPE_SAT").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.18),
                    shoulder: std::env::var("LOGSCOPE_SHOULDER").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0),
                    // per-scene BLOOM trim — Old Library + Cloudscape were over-blooming (detail washed out)
                    threshold: gthr * 1.0, bloom_amt: gbloom * 0.85 * match name { "Old Library" => 0.5, "Cloudscape" => 0.42, "Reading Room" => 0.5, "Mansion Garden" => 0.26, _ => 1.0 }, texscale: dtexscale, bump: dbump,
                    caustic, godray_amt: gr_amt, godray_weight: gr_w, godray_decay: gr_dec,
                    pearl, pearl_variant, pearl_pos, pearl_r, aspect: rect.width() / rect.height().max(1.0),
                    nacre: std::env::var("LOGSCOPE_NACRE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.4),
                    underwater,
                    defocus: if self.backdrop { 0.85 } else { 0.0 }, // frost strength when it's the glass-UI backdrop
                    frost_a: self.frost_a, frost_b: self.frost_b,    // ...but only within the chrome bands
                    cam_warp: [view_cam.yaw, view_cam.pitch],         // 3D-aware underwater field follows the look angle
                    // backdrop is opaque so no undefined framebuffer shows through; fill empty sky with the theme bg
                    opaque: if self.backdrop { 1.0 } else { 0.0 },
                    bg: { let b = theme::active().bg; [b.r() as f32 / 255.0 * 0.7, b.g() as f32 / 255.0 * 0.7, b.b() as f32 / 255.0 * 0.7] },
                    // DEPTH-OF-FIELD: focus on the content (orbit distance), defocus the far background. Tunable.
                    dof: std::env::var("LOGSCOPE_DOF").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.7),
                    focus: view_cam.dist,
                    // SHADOWS: ortho shadow map along the sun, CENTERED ON THE CAMERA TARGET (follows the view)
                    // with a doubled radius for wider coverage. Toggle off with LOGSCOPE_SHADOWS=0.
                    light_viewproj: crate::gpuscene::light_view_proj(
                        [sun.x, sun.y, sun.z],
                        [view_cam.target.x, view_cam.target.y, view_cam.target.z], 128.0),
                    shadow_on: if std::env::var("LOGSCOPE_SHADOWS").map(|v| v == "0").unwrap_or(false) { 0.0 } else { 1.0 },
                    // COLORED FILL LIGHT: a soft directional fill from the off-sun side, tinted by the scene's
                    // ambient sky colour — so the shadowed side picks up the scene's hue (aqua underwater, green
                    // under canopy, moonlit blue, …) instead of going flat-grey. Complements the per-theme sun.
                    fill_dir: { let (x, y, z) = (-sun.x * 0.8 + 0.25, 0.5, -sun.z * 0.8 - 0.15);
                                let l = (x * x + y * y + z * z).sqrt().max(1e-4); [x / l, y / l, z / l] },
                    fill_col: { let s = c3(lsky); [s[0] * 0.55, s[1] * 0.55, s[2] * 0.55] },
                    ..Default::default()
                };
                p.add(crate::glow_scene::callback(rect, u, verts));
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(40)); // strands ripple + runes glow
        }

        // For the pearl/fiber-optic scene the 3D light-cords + glossy pearls ARE the graph, so the flat
        // 2D edges and opaque dots below are suppressed to thin rings — they'd otherwise mask the bodies.
        let pearl_scene = theme::active().name == "Atlantis";
        // edges (dim if either endpoint is pruned)
        if !pearl_scene {
            for nd in &g.nodes {
                if let (Some(pid), Some((sx, sy)), Some((px, py))) = (nd.parent, screen(nd.id), nd.parent.and_then(screen)) {
                    let dim = self.pruned.contains(&nd.id) || self.pruned.contains(&pid);
                    let col = if dim { theme::LINE().linear_multiply(0.4) } else { theme::LINE() };
                    p.line_segment([egui::pos2(px, py), egui::pos2(sx, sy)], egui::Stroke::new(1.0, col));
                }
            }
        }
        // nodes: color by section, size by depth × token "heat" (hot turns fatter), pruned = hollow
        let near = proj.iter().map(|q| q.3).fold(f32::MAX, f32::min);
        let far = proj.iter().map(|q| q.3).fold(0.0_f32, f32::max);
        let span = (far - near).max(0.001);
        let time = ui.input(|i| i.time) as f32;
        let acc = theme::accent();
        let _acc_a = |a: u8| egui::Color32::from_rgba_unmultiplied(acc.r(), acc.g(), acc.b(), a); // (ripple rings removed)
        for &(id, sx, sy, depth) in &proj {
            let nd = &g.nodes[id];
            let t = ((depth - near) / span).clamp(0.0, 1.0); // 0 near, 1 far
            let hot = (nd.tokens as f32 / max_tok).clamp(0.0, 1.0);
            let radius = ((6.5 - 4.0 * t) * (1.0 + 0.7 * hot)).max(2.0);
            let sec = palette[seg_of[id] % palette.len()];
            let col = sec.linear_multiply(1.0 - 0.5 * t);
            let prn = self.pruned.contains(&id);
            let pt = egui::pos2(sx, sy);
            // (removed: the "gravitational air ripple" — 3 concentric circle_stroke rings that breathed outward to
            //  radius*7. For a near node those became huge room-sized skinny rings overlaying the floor across every
            //  scene. Significance is already carried by node size/glow; the rings read as stray geometry.)
            let _ = (hot, prn);
            if self.region.as_ref().map_or(false, |r| r.contains(&id)) { p.circle_stroke(pt, radius + 5.0, egui::Stroke::new(1.6, egui::Color32::from_rgb(0xc9, 0x8f, 0xff))); } // :best region
            if self.walk_focus == Some(id) { p.circle_stroke(pt, radius + 4.0, egui::Stroke::new(2.0, theme::AMBER())); }
            if self.branch_from == Some(id) { p.circle_stroke(pt, radius + 6.5, egui::Stroke::new(2.0, egui::Color32::from_rgb(0xc9, 0x8f, 0xff))); }
            if self.active_leaf == Some(id) { p.circle_stroke(pt, radius + 2.0, egui::Stroke::new(1.5, theme::CYAN())); } // live tip
            if prn {
                p.circle_stroke(pt, radius, egui::Stroke::new(1.0, col.linear_multiply(0.5)));
            } else if pearl_scene {
                // outline only — let the 3D glossy pearl behind it show through
                p.circle_stroke(pt, radius, egui::Stroke::new(1.0, col.linear_multiply(0.7)));
                if nd.turn == crate::convograph::Turn::User { p.circle_stroke(pt, radius + 1.5, egui::Stroke::new(1.0, theme::CYAN().linear_multiply(0.6))); }
            } else {
                p.circle_filled(pt, radius, col);
                if nd.turn == crate::convograph::Turn::User { p.circle_stroke(pt, radius + 1.0, egui::Stroke::new(1.0, theme::INK().linear_multiply(0.5))); }
            }
            if t < 0.42 && !prn {
                // hand off to the 3D resonance text up close: the 2D caption lives in a mid band, fading in
                // around the approach and fading back out as the pearl's own text resolves (t→0).
                let label_fade = ((t - 0.12) / 0.10).clamp(0.0, 1.0) * (1.0 - ((t - 0.30) / 0.12).clamp(0.0, 1.0));
                if label_fade > 0.02 {
                    let base = if t < 0.2 { theme::INK() } else { theme::DIM() };
                    p.text(egui::pos2(sx + radius + 5.0, sy), egui::Align2::LEFT_CENTER,
                        format!("{}t · {}", nd.tokens, nd.preview.chars().take(22).collect::<String>()),
                        egui::FontId::proportional(11.0), base.gamma_multiply(label_fade));
                }
            }
        }
        // glowing identicon runes inscribed on each near stone — the node's short-id, encoded + emissive
        {
            let tt = ui.input(|i| i.time) as f32;
            let grid = 5usize;
            for &(id, sx, sy, depth) in &proj {
                let frac = ((depth - near) / span).clamp(0.0, 1.0);
                if frac > 0.5 { continue; }                 // only legible on the near stones
                let half = (0.26 * scale / depth) * 0.85;   // rune box ≈ the stone's projected size
                if half < 6.0 { continue; }
                // quiet the runes as the camera closes in: they HAND OFF to the 3D resonance text up close.
                // Strongest at mid-approach, gone very near (text takes over) and gone far.
                let approach = ((frac - 0.05) / 0.16).clamp(0.0, 1.0); // 0 when very near → runes quiet
                if approach < 0.03 { continue; }
                let cell = (half * 2.0) / grid as f32;
                let pulse = 0.55 + 0.45 * (tt * 1.7 + id as f32 * 0.6).sin();
                let glow = (if self.active_leaf == Some(id) { theme::CYAN() } else { theme::AMBER() })
                    .gamma_multiply(approach * (1.0 - frac) * pulse);
                let (ox, oy) = (sx - half, sy - half);
                for (cxx, cyy) in crate::mesh3d::rune_cells(crate::vimmode::short_id_seed(id), grid) {
                    let p0 = egui::pos2(ox + cxx as f32 * cell + cell * 0.1, oy + cyy as f32 * cell + cell * 0.1);
                    p.rect_filled(egui::Rect::from_min_size(p0, egui::vec2(cell * 0.8, cell * 0.8)), theme::r(1), glow);
                }
            }
        }
        // DEBUG (LOGSCOPE_DEBUG_LABELS): label the NEAREST 10 scene meshes on-screen — mesh index + its max
        // dimension in world units — so any element (e.g. a stray ring) can be identified by eye and cross-
        // referenced with the /tmp/scene_labels_<theme>.txt dump. Rebuilds the env once/frame; debug-only.
        if std::env::var("LOGSCOPE_DEBUG_LABELS").is_ok() {
            let mut labels: Vec<(usize, f32, f32, f32, f32)> = Vec::new(); // (idx, sx, sy, depth, max_dim)
            crate::scene3d::with_env(theme::active().name, ui.input(|i| i.time) as f32, |meshes| {
                for (i, m) in meshes.iter().enumerate() {
                    if m.verts.is_empty() { continue; }
                    let (mut sxw, mut syw, mut szw) = (0.0f32, 0.0f32, 0.0f32);
                    let (mut lo, mut hi) = (m.verts[0], m.verts[0]);
                    for v in &m.verts {
                        sxw += v.x; syw += v.y; szw += v.z;
                        lo.x = lo.x.min(v.x); lo.y = lo.y.min(v.y); lo.z = lo.z.min(v.z);
                        hi.x = hi.x.max(v.x); hi.y = hi.y.max(v.y); hi.z = hi.z.max(v.z);
                    }
                    let nn = m.verts.len() as f32;
                    let c = crate::spatial::Vec3::new(sxw / nn, syw / nn, szw / nn);
                    if let Some((px, py, depth)) = view_cam.project(c, cx, cy, scale) {
                        let md = (hi.x - lo.x).max(hi.y - lo.y).max(hi.z - lo.z);
                        labels.push((i, px, py, depth, md));
                    }
                }
            });
            labels.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));
            labels.truncate(10);
            for (i, px, py, _d, md) in labels {
                let pt = egui::pos2(px, py);
                p.circle_stroke(pt, 5.0, egui::Stroke::new(1.5, egui::Color32::from_rgb(0xff, 0x4d, 0x6d)));
                let lab = format!("#{} {:.0}u", i, md);
                let fid = egui::FontId::monospace(12.0);
                p.text(pt + egui::vec2(8.0, -5.0), egui::Align2::LEFT_CENTER, &lab, fid.clone(), egui::Color32::BLACK);
                p.text(pt + egui::vec2(7.0, -6.0), egui::Align2::LEFT_CENTER, &lab, fid, egui::Color32::from_rgb(0xff, 0xe0, 0x55));
            }
        }

        // click → nearest node (focus it; opt out of auto-follow so it stays put)
        if resp.clicked() {
            if let Some(ptr) = resp.interact_pointer_pos() {
                let mut best: Option<(usize, f32)> = None;
                for &(id, sx, sy, _) in &proj {
                    let d2 = (sx - ptr.x).powi(2) + (sy - ptr.y).powi(2);
                    if best.map(|(_, bd)| d2 < bd).unwrap_or(true) { best = Some((id, d2)); }
                }
                if let Some((id, d2)) = best {
                    if d2 < 26.0 * 26.0 {
                        // glide along the graph's shortest path from the current focus to the clicked node
                        let from = self.walk_focus.unwrap_or(id);
                        let path = graph_path(&g, from, id);
                        if path.len() > 1 { self.cam_pan_path = path; self.cam_pan_t0 = Some(std::time::Instant::now()); }
                        else { self.orbit.target = pos[id]; }
                        self.walk_focus = Some(id); self.spatial_auto = false; ui.ctx().request_repaint();
                    }
                }
            }
        }

        // focused-node preview strip with prune/restore for that turn (skipped when used as a backdrop)
        if !self.backdrop {
        ui.add_space(4.0);
        if let Some(id) = self.walk_focus.filter(|i| *i < self.msgs.len()) {
            let (who, text) = { let m = &self.msgs[id]; (if m.role == Role::User { "you" } else { "assistant" }, m.text().chars().take(280).collect::<String>()) };
            let prn = self.pruned.contains(&id);
            egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(8), inner_margin: egui::Margin::same(theme::PAD_SM),
                stroke: egui::Stroke::new(1.0, theme::AMBER()), ..Default::default() }
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{who} · turn {}/{}", id + 1, n)).color(theme::AMBER()).small().strong());
                    if prn {
                        if ui.small_button("↩ Restore").clicked() { self.pruned.remove(&id); ui.ctx().request_repaint(); }
                        ui.label(RichText::new("· pruned, out of context").color(theme::DIM()).small());
                    } else if ui.small_button("✂ Prune").on_hover_text("Drop this turn from the model's context").clicked() {
                        self.pruned.insert(id); ui.ctx().request_repaint();
                    }
                    if self.branch_from == Some(id) {
                        ui.label(RichText::new("⑂ next message forks here").color(theme::CYAN()).small().strong());
                        if ui.small_button("✕").on_hover_text("Cancel branch").clicked() { self.branch_from = None; }
                    } else if ui.small_button("⑂ Branch here").on_hover_text("Reply as a side-thread off this turn — the next message you send forks here instead of continuing the tip").clicked() {
                        self.branch_from = Some(id); ui.ctx().request_repaint();
                    }
                });
                ui.label(RichText::new(text).color(if prn { theme::DIM() } else { theme::INK() }).small());
            });
        } else {
            ui.label(RichText::new("Click a node to read its turn here. ⊹ Overview reframes; ✂ prunes to free context.").color(theme::FAINT()).small());
        }
        }
    }

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
    fn attach_path(&mut self, path: &str, ui: &egui::Ui) {
        let p = shellexpand_home(path);
        match std::fs::read(&p) {
            Ok(bytes) => {
                let cap = 64 * 1024;
                let shown = &bytes[..bytes.len().min(cap)];
                let text = String::from_utf8_lossy(shown);
                let name = std::path::Path::new(&p).file_name().and_then(|s| s.to_str()).unwrap_or("file");
                if !self.input.is_empty() && !self.input.ends_with('\n') { self.input.push('\n'); }
                self.input.push_str(&format!("\n--- {name} ---\n```\n{text}\n```\n"));
                if bytes.len() > cap { self.input.push_str(&format!("(truncated; showed first {cap} bytes)\n")); }
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
            "j/k move · h/l branch · g/G ends · i/I zoom · z+hjkl rotate · o fit · / find · : cmd · b branch · a write · ? help".into()
        } else {
            "j/k move · h/l branch · g/G ends · / find · : cmd · b branch · a write · ? help".into()
        }
    }

    fn run_command(&mut self, cmd: crate::vimmode::Command, ui: &egui::Ui) {
        use crate::vimmode::Command;
        match cmd {
            Command::Branch => { if let Some(f) = self.walk_focus { self.branch_from = Some(f); } }
            Command::Attach(p) => self.attach_path(&p, ui),
            Command::Goto(n) => { let i = n.saturating_sub(1).min(self.msgs.len().saturating_sub(1)); self.set_focus(Some(i)); }
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
                let base = self.input.trim().to_string();
                if !base.is_empty() {
                    let parent = self.walk_focus.or(self.active_leaf);
                    for v in crate::consensus::fuzz(&base, n, now_unix()) { self.queue.push_back((v, parent)); }
                    self.input.clear();
                }
            }
            // :fuzzd N — like :fuzz, but each branch carries a different BEHAVIORAL directive (strategy/
            // persona/constraint…) instead of a lexical reword, so the fan explores different approaches
            // to the same task. The directive name could later label each branch in the spatial view.
            Command::FuzzDirective(n) => {
                let base = self.input.trim().to_string();
                if !base.is_empty() {
                    let parent = self.walk_focus.or(self.active_leaf);
                    for (v, _name) in crate::directives::fanout_user(&base, n, now_unix()) { self.queue.push_back((v, parent)); }
                    self.input.clear();
                }
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
                    self.fan = Some(Fan { kind: FanKind::Continue, rounds_left: depth });
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
                    self.fan = Some(Fan { kind: FanKind::Fuzzy { width, template: task }, rounds_left: depth });
                    self.fan_step();
                    self.note = format!("fuzzy-all depth {depth} × width {width} — fanning (:stop to cancel)");
                }
            }
            Command::Stop => { let n = self.queue.len(); self.queue.clear(); self.fan = None; self.note = format!("stopped — cleared {n} queued, fan cancelled"); }
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
            Command::Compact(arg) => {
                // active lineage (tip → root), reversed to chronological
                let mut chain = Vec::new();
                let mut cur = self.active_leaf;
                while let Some(i) = cur { chain.push(i); cur = self.msgs.get(i).and_then(|m| m.parent); }
                chain.reverse();
                let turns: Vec<(String, String)> = chain.iter().filter_map(|&i| self.msgs.get(i))
                    .map(|m| ((if m.role == Role::Assistant { "assistant" } else { "user" }).to_string(), m.text().to_string()))
                    .collect();
                if turns.len() < 2 { self.note = ":compact — need at least two turns of history to compact".into(); }
                else {
                    let ratio = arg.trim().parse::<f32>().ok().filter(|r| *r > 0.0 && *r < 1.0).unwrap_or(0.3);
                    let plan = crate::refresh::plan(&turns, 4, ratio);
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
                    r.request_focus();
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
                        let ctx = crate::cmdpalette::ArgCtx { models: Vec::new(), refs: vec!["-1".into(), "-2".into(), "-3".into()] };
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
                    r.request_focus();
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
                        ui.label(RichText::new(&self.note).color(theme::GREEN()).small());
                    } else {
                        ui.label(RichText::new(self.modal_hint()).color(theme::FAINT()).small());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("?").on_hover_text("keybindings (or press ?)").clicked() { self.show_help = !self.show_help; }
                        if let Some(f) = self.walk_focus {
                            ui.label(RichText::new(format!("turn {}/{}", f + 1, self.msgs.len())).color(theme::DIM()).small());
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

    pub fn ui(&mut self, ui: &mut egui::Ui, model: &mut ModelState, doc: &crate::analysis::Doc, infer: &Inference, model_loaded: bool) {
        self.sync_worker(infer);
        self.check_restore(); // first frame: offer to recover a crashed/closed conversation
        if self.pending_cancel { infer.cancel(); self.pending_cancel = false; }
        let streaming = self.stream.is_some();
        if streaming { ui.ctx().request_repaint(); }
        self.handle_modal(ui, infer, doc); // modal (vim) keys — both views; default Insert keeps typing free

        // ── top bar: status · threading toggle · context toggle · clear ──
        ui.horizontal(|ui| {
            let (badge, bc) = if model_loaded { ("model loaded", theme::GREEN()) } else { ("no model — Load a GGUF up top", theme::DIM()) };
            theme::chip(ui, badge, bc);
            ui.label(RichText::new(self.backend.label()).color(theme::FAINT()).small());
            if let Some(s) = &self.stream {
                let secs = s.started.elapsed().as_secs_f32().max(0.001);
                let toks = s.tokens;
                theme::pulse_dot(ui, theme::AMBER());
                let debug = std::env::var("LOGSCOPE_DEBUG").is_ok();
                if debug {
                    ui.label(RichText::new(format!("{toks} tok · {:.1} tok/s", toks as f32 / secs)).color(theme::AMBER()).small());
                } else if let Some(pred) = crate::estimate::predict(&self.gen_samples, self.last_prompt_tokens, 0.35) {
                    let pct = (crate::estimate::progress(toks, &pred) * 100.0).round() as u32;
                    ui.label(RichText::new(format!("decoding… ~{pct}%")).color(theme::AMBER()).small())
                        .on_hover_text(format!("estimated from {} prior runs{}", self.gen_samples.len(), if pred.from_similar { " with similar prompts" } else { "" }));
                } else {
                    ui.label(RichText::new(format!("decoding… {toks} tokens")).color(theme::AMBER()).small());
                }
                // Display chunk boundary: with auto-continue the reply decodes continuously past the
                // per-chunk size, so mark which chunk it's flowing through (no decode stop).
                let chunk = self.ctx.max_tokens.max(1) as usize;
                if self.auto_continue && toks >= chunk {
                    ui.label(RichText::new(format!("· chunk {}/{}", toks / chunk + 1, 1 + MAX_AUTO_CONTINUE)).color(theme::FAINT()).small());
                }
            } else if let Some((t, secs)) = self.last {
                if std::env::var("LOGSCOPE_DEBUG").is_ok() {
                    ui.label(RichText::new(format!("· {t} tok · {:.1} tok/s", t as f32 / secs.max(0.001))).color(theme::FAINT()).small());
                } else {
                    ui.label(RichText::new(format!("· {t} tokens")).color(theme::FAINT()).small());
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Clear").clicked() { self.msgs.clear(); self.last = None; self.active_leaf = None; self.branch_from = None; self.pruned.clear(); self.queue.clear(); self.fold_pos.clear(); self.region = None; self.fan = None; self.count_pending = None; }
                ui.add_space(6.0);
                // PEARL: toggle the in-scene refractive composer pearl + pick its optical variant
                if self.pearl_on {
                    let names = ["balanced", "fine-crackle", "milky", "clear-glass"];
                    let cur = names[(self.pearl_variant as usize).min(3)];
                    crate::ui::combo(ui, "pearl_variant", cur, 116.0, |ui| {
                        for (i, n) in names.iter().enumerate() {
                            if ui.selectable_label(self.pearl_variant as usize == i, *n).clicked() { self.pearl_variant = i as u8; }
                        }
                    });
                }
                theme::switch(ui, &mut self.pearl_on, theme::CYAN());
                ui.label(RichText::new("Pearl").size(10.5).color(if self.pearl_on { theme::CYAN() } else { theme::DIM() }))
                    .on_hover_text("Cast the refractive composer pearl into the scene (physically-shaded: refraction · fresnel · thin-film).");
                ui.add_space(6.0);
                theme::switch(ui, &mut self.threading, theme::CYAN());
                ui.label(RichText::new("Thread").size(10.5).color(if self.threading { theme::CYAN() } else { theme::DIM() }))
                    .on_hover_text("Thread prior turns into the prompt (multi-turn). Off: each message is sent alone with the context.");
                ui.add_space(6.0);
                theme::switch(ui, &mut self.attach_log, theme::GREEN());
                ui.label(RichText::new("Log").size(10.5).color(if self.attach_log { theme::GREEN() } else { theme::DIM() }))
                    .on_hover_text("Include a bounded summary of the current log (line count, top repeated patterns, last lines) in the system prompt.");
                ui.add_space(6.0);
                theme::switch(ui, &mut self.fix_mode, theme::AMBER());
                ui.label(RichText::new("Fix").size(10.5).color(if self.fix_mode { theme::AMBER() } else { theme::DIM() }))
                    .on_hover_text("Step-up fix submode: the assistant names the fault and emits a concrete fix (file or diff) you can save, instead of just discussing it. Pairs with Log.");
                ui.add_space(6.0);
                if ui.selectable_label(self.show_context, RichText::new("☰ Panels").color(theme::DIM())).on_hover_text("Files · Graph · Context · Prompting").clicked() {
                    self.show_context = !self.show_context;
                }
                ui.add_space(6.0);
                let vlabel = if self.spatial_view { "◫ Spatial" } else { "▤ Linear" };
                if ui.selectable_label(self.spatial_view, RichText::new(vlabel).color(if self.spatial_view { theme::CYAN() } else { theme::DIM() }))
                    .on_hover_text("Switch the main area between the linear chat and the 3D message graph").clicked() {
                    let next = !self.spatial_view;
                    self.set_view(next, ui);
                }
            });
        });

        // prominent, always-visible modal indicator (mode pill + live :/​/ input or key hint)
        self.modal_banner(ui);
        if self.request_smaller_model {
            self.request_smaller_model = false;
            if let Some(i) = crate::model::smallest_local(&model.items) {
                model.selected = i;
                let name = model.current();
                self.note = format!("🚩 diagnosing the bad jump with {name} (smallest local model) — send to run.");
            } else {
                self.note = "🚩 bad jump staged, but no local model is available for the diagnosis — load a small GGUF.".into();
            }
        }
        self.help_overlay(ui);

        // artifacts from the latest reply (consumed by the bottom panel AND the Files sidebar page)
        let last_text = self.msgs.iter().rev().find(|m| m.role == Role::Assistant).map(|m| m.text().to_string());
        let arts = last_text.as_deref().map(crate::workspace::parse_artifacts).unwrap_or_default();

        // poll a running sandbox verify (cargo check) job
        if let Some(job) = &mut self.verify_job {
            if job.result.is_none() {
                match job.rx.try_recv() {
                    Ok(r) => job.result = Some(r),
                    Err(std::sync::mpsc::TryRecvError::Empty) => { ui.ctx().request_repaint(); }
                    Err(_) => job.result = Some((false, "verify worker disconnected".into(), false)),
                }
            }
        }

        // ── right sidebar: per-conversation subpages behind a button rail (Files · Context · Prompting) ──
        if self.show_context {
            egui::SidePanel::right("chat_right").resizable(true).default_width(330.0).show_inside(ui, |ui| {
                ui.add_space(theme::PAD_XS as f32);
                ui.horizontal(|ui| {
                    if ui.selectable_label(self.right_page == RightPage::Files, RichText::new("📄 Files").small()).clicked() { self.right_page = RightPage::Files; }
                    if ui.selectable_label(self.right_page == RightPage::Graph, RichText::new("⌗ Graph").small()).clicked() { self.right_page = RightPage::Graph; }
                    if ui.selectable_label(self.right_page == RightPage::Context, RichText::new("⚙ Context").small()).clicked() { self.right_page = RightPage::Context; }
                    if ui.selectable_label(self.right_page == RightPage::Prompting, RichText::new("✦ Prompting").small()).clicked() { self.right_page = RightPage::Prompting; }
                });
                ui.separator();
                // floating context cards — the freshest code block, the loaded file, and salient
                // keywords, kept next to the chat regardless of which subpage is open.
                {
                    let code = self.msgs.iter().rev()
                        .map(|m| crate::verdict::extract_code(m.text()))
                        .find(|c| !c.trim().is_empty());
                    let kw_src: String = self.msgs.iter().rev().take(4).map(|m| m.text()).collect::<Vec<_>>().join(" ");
                    let kws: Vec<String> = crate::convograph::keywords(&kw_src).into_iter().take(6).collect();
                    let has_file = !doc.name.is_empty();
                    if code.is_some() || has_file || !kws.is_empty() {
                        egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(8), inner_margin: egui::Margin::same(theme::PAD_SM),
                            outer_margin: egui::Margin { bottom: theme::PAD_SM, ..Default::default() }, stroke: egui::Stroke::new(1.0, theme::LINE()), ..Default::default() }
                        .show(ui, |ui| {
                            ui.label(RichText::new("CONTEXT CARDS").color(theme::DIM()).small().strong());
                            if has_file {
                                ui.add_space(3.0);
                                ui.label(RichText::new(format!("📄 {}", doc.name)).color(theme::CYAN()).small());
                            }
                            if let Some(code) = &code {
                                ui.add_space(4.0);
                                let shown: String = code.lines().take(6).collect::<Vec<_>>().join("\n");
                                let more = code.lines().count() > 6;
                                egui::Frame { fill: theme::TRACK_BG(), corner_radius: theme::r(6), inner_margin: egui::Margin::same(6), ..Default::default() }
                                    .show(ui, |ui| {
                                        ui.label(RichText::new(shown).monospace().size(10.5).color(theme::INK()));
                                        if more { ui.label(RichText::new("…").color(theme::FAINT()).small()); }
                                    });
                                if ui.small_button("⧉ copy code").clicked() { ui.ctx().copy_text(code.clone()); }
                            }
                            if !kws.is_empty() {
                                ui.add_space(5.0);
                                ui.horizontal_wrapped(|ui| { for k in &kws { theme::chip(ui, k, theme::AMBER()); } });
                            }
                        });
                    }
                }
                egui::ScrollArea::vertical().id_salt("right_page").auto_shrink([false, false]).show(ui, |ui| {
                    match self.right_page {
                        RightPage::Context => {
                            ui.label(RichText::new("CONTEXT (system prompt)").color(theme::DIM()).small().strong());
                            ui.add(egui::TextEdit::multiline(&mut self.context).desired_rows(4).desired_width(f32::INFINITY)
                                .hint_text("System prompt / persistent context for every turn…"));
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("backend").color(theme::DIM()).small());
                                egui::ComboBox::from_id_salt("backend").width(180.0).selected_text(self.backend.label())
                                    .show_ui(ui, |ui| { for k in BackendKind::ALL { ui.selectable_value(&mut self.backend, k, k.label()); } });
                            });
                            ui.add(egui::Slider::new(&mut self.ctx.n_ctx, 512..=32768).step_by(256.0).text("ctx window"))
                                .on_hover_text("Total context (prompt + reply). Bigger = more memory; a long paste is trimmed to fit this.");
                            ui.add(egui::Slider::new(&mut self.ctx.max_tokens, 64..=4096).step_by(64.0).text("reply budget"));
                            ui.add(egui::Slider::new(&mut self.ctx.temperature, 0.0..=1.5).text("temperature"));
                            ui.add(egui::Slider::new(&mut self.ctx.repeat_penalty, 1.0..=1.5).text("repeat penalty"));
                            ui.add(egui::Slider::new(&mut self.ctx.min_p, 0.0..=0.3).step_by(0.01).text("min-p"));
                            ui.add_enabled(self.threading, egui::Slider::new(&mut self.ctx.history_turns, 1..=32).text("history turns"));
                            let used: usize = est(&self.context) + self.msgs.iter().map(|m| est(m.text()) + 4).sum::<usize>();
                            let budget = self.ctx.n_ctx.saturating_sub(self.ctx.max_tokens).max(1) as usize;
                            let frac = (used as f32 / budget as f32).min(1.0);
                            let col = if used > budget { theme::RED() } else if frac > 0.8 { theme::YELLOW() } else { theme::CYAN() };
                            theme::usage_bar_anim(ui, "ctx_usage", 280.0, frac, col);
                            ui.label(RichText::new(format!("~{used} / {budget} tok · model: {}", model.current())).color(theme::DIM()).small());
                        }
                        RightPage::Prompting => {
                            ui.label(RichText::new("PROMPTING MODE").color(theme::DIM()).small().strong());
                            ui.add_space(4.0);
                            for m in crate::coordinator::Mode::all() {
                                let on = self.coord_mode == m;
                                if ui.selectable_label(on, RichText::new(m.label()).color(if on { theme::CYAN() } else { theme::INK() }).strong()).clicked() {
                                    self.coord_mode = m;
                                    // aggressive pulls in the spatial view; barebones returns to the linear chat
                                    match m { crate::coordinator::Mode::Aggressive => self.set_view(true, ui),
                                              crate::coordinator::Mode::Barebones => self.set_view(false, ui),
                                              _ => {} }
                                }
                                ui.label(RichText::new(m.blurb()).color(theme::DIM()).small());
                                ui.add_space(6.0);
                            }
                            ui.separator();
                            ui.label(RichText::new("Coordinator modes fan one task out to parallel workers over the same loaded model — no extra models needed. Task decomposition + execution wiring is in progress; selecting a mode here records the intent.").color(theme::FAINT()).small());
                        }
                        RightPage::Graph => {
                            ui.label(RichText::new("CONVERSATION GRAPH").color(theme::DIM()).small().strong());
                            ui.label(RichText::new("Click a node to jump the chat there; ◀ ▶ dolly the camera through it.").color(theme::FAINT()).small());
                            let mut g = self.convo_graph(self.msgs.len());
                            let budget = self.ctx.n_ctx.saturating_sub(self.ctx.max_tokens).max(1) as usize;
                            let max_turns = if self.threading { self.ctx.history_turns * 2 + 1 } else { 1 };
                            let (in_ctx, used) = g.mark_window_ex(budget, est(self.sys()) + 8, 8, max_turns);
                            let n = g.nodes.len();
                            if n == 0 {
                                ui.label(RichText::new("No turns yet — start a conversation to see the spine form.").color(theme::DIM()).small());
                            } else {
                                ui.horizontal(|ui| {
                                    let cur = self.walk_focus.unwrap_or(0);
                                    if ui.add_enabled(cur > 0, egui::Button::new(RichText::new("◀").small())).on_hover_text("Previous turn").clicked() {
                                        let t = cur.saturating_sub(1); self.walk_focus = Some(t); self.scroll_chat_to = Some(t);
                                    }
                                    ui.label(RichText::new(self.walk_focus.map(|i| format!("{}/{n}", i + 1)).unwrap_or_else(|| format!("–/{n}"))).color(theme::DIM()).small());
                                    if ui.add_enabled(cur + 1 < n, egui::Button::new(RichText::new("▶").small())).on_hover_text("Next turn").clicked() {
                                        let t = (cur + 1).min(n - 1); self.walk_focus = Some(t); self.scroll_chat_to = Some(t);
                                    }
                                    if ui.button(RichText::new("⤓ latest").small()).clicked() { let t = n - 1; self.walk_focus = Some(t); self.scroll_chat_to = Some(t); }
                                });
                                ui.add_space(4.0);
                                let row_h = 30.0;
                                let content_h = n as f32 * row_h + 12.0;
                                let viewport_h = 240.0_f32;
                                // advance the camera toward the focused node — critically damped, cuts on a big jump
                                let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.05);
                                if let Some(idx) = self.walk_focus {
                                    self.walk.focus(idx, row_h, viewport_h, content_h, 3.0, viewport_h * 0.9);
                                    self.walk.tick(dt);
                                    if !self.walk.settled() { ui.ctx().request_repaint(); }
                                }
                                let mut clicked_node: Option<usize> = None;
                                egui::ScrollArea::vertical().id_salt("graph_view").max_height(viewport_h)
                                    .vertical_scroll_offset(self.walk.top)
                                    .show(ui, |ui| {
                                        let (resp, p) = ui.allocate_painter(egui::vec2(ui.available_width().max(140.0), content_h), egui::Sense::click());
                                        let rect = resp.rect;
                                        let cx = rect.left() + 16.0;
                                        let top = rect.top() + 8.0;
                                        for nd in &g.nodes {
                                            let y = top + nd.id as f32 * row_h;
                                            if let Some(pid) = nd.parent {
                                                let py = top + pid as f32 * row_h;
                                                let edge = if nd.in_context && g.nodes[pid].in_context { theme::CYAN().linear_multiply(0.5) } else { theme::LINE() };
                                                p.line_segment([egui::pos2(cx, py), egui::pos2(cx, y)], egui::Stroke::new(1.5, edge));
                                            }
                                            let base = if nd.turn == crate::convograph::Turn::User { theme::CYAN() } else { theme::GREEN() };
                                            let dot = if nd.in_context { base } else { base.linear_multiply(0.30) };
                                            let focused = self.walk_focus == Some(nd.id);
                                            if focused { p.circle_stroke(egui::pos2(cx, y), 8.0, egui::Stroke::new(2.0, theme::AMBER())); }
                                            if nd.pruned { p.circle_stroke(egui::pos2(cx, y), 5.0, egui::Stroke::new(1.0, theme::DIM())); }
                                            else { p.circle_filled(egui::pos2(cx, y), 5.0, dot); }
                                            let tcol = if nd.pruned { theme::FAINT() } else if focused { theme::AMBER() } else if nd.in_context { theme::INK() } else { theme::DIM() };
                                            let who = if nd.turn == crate::convograph::Turn::User { "you" } else { "asst" };
                                            p.text(egui::pos2(cx + 14.0, y), egui::Align2::LEFT_CENTER,
                                                format!("{who} · {}t  {}{}", nd.tokens, if nd.pruned { "⌫ " } else { "" }, nd.preview),
                                                egui::FontId::proportional(11.0), tcol);
                                        }
                                        if resp.clicked() {
                                            if let Some(pos) = resp.interact_pointer_pos() {
                                                let i = (((pos.y - top) / row_h).round() as i64).clamp(0, n as i64 - 1) as usize;
                                                clicked_node = Some(i);
                                            }
                                        }
                                    });
                                if let Some(i) = clicked_node { self.walk_focus = Some(i); self.scroll_chat_to = Some(i); }
                                ui.add_space(8.0);
                                ui.separator();
                                ui.label(RichText::new(format!("{n} turns stored · {in_ctx} in window · ~{used}/{budget} tok")).color(theme::DIM()).small());
                                ui.label(RichText::new("The amber ring is the walkthrough focus; click or step jumps the chat there. Branch forks appear once multiple-solution mapping is wired.").color(theme::FAINT()).small());
                            }
                        }
                        RightPage::Files => {
                            ui.label(RichText::new("GENERATED FILES").color(theme::DIM()).small().strong());
                            ui.add_space(4.0);
                            if arts.is_empty() {
                                ui.label(RichText::new("No files in the latest reply yet.").color(theme::DIM()).small());
                            } else {
                                ui.label(RichText::new(format!("{} file(s) from the latest reply:", arts.len())).color(theme::DIM()).small());
                                for a in &arts { ui.label(RichText::new(format!("• {}", a.path)).color(theme::FAINT()).small()); }
                                ui.add_space(6.0);
                                // ── sandbox oracle: cargo check the generated files in an isolated copy ──
                                let running = self.verify_job.as_ref().map(|j| j.result.is_none()).unwrap_or(false);
                                let rust = arts.iter().any(|a| a.path.ends_with(".rs"));
                                ui.horizontal(|ui| {
                                    if ui.add_enabled(!running && rust, egui::Button::new(RichText::new("⌗ Verify build").small()))
                                        .on_hover_text("Offline-first: scaffold the files into a throwaway cargo project and run `cargo check --offline` in an isolated copy.")
                                        .clicked()
                                    {
                                        let files: Vec<(String, String)> = arts.iter().map(|a| (a.path.clone(), a.content.clone())).collect();
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
                                    ui.checkbox(&mut self.verify_net, RichText::new("fetch deps").small())
                                        .on_hover_text("Off = hermetic (offline only). On = if a dep isn't cached, fetch it from crates.io once (then cached).");
                                });
                                if !rust { ui.label(RichText::new("(no .rs files to check)").color(theme::FAINT()).small()); }
                                if let Some(job) = &self.verify_job {
                                    match &job.result {
                                        None => { ui.label(RichText::new(format!("checking… {:.0}s", job.started.elapsed().as_secs_f32())).color(theme::AMBER()).small()); }
                                        Some((passed, output, sandboxed)) => {
                                            let (txt, col, note) = match crate::sandbox::classify(*passed, output) {
                                                crate::sandbox::Verdict::Pass => ("✓ compiles", theme::GREEN(), ""),
                                                crate::sandbox::Verdict::CompileError => ("✗ build failed", theme::RED(), ""),
                                                crate::sandbox::Verdict::DepsUnavailable => ("⚠ dependencies not cached", theme::AMBER(), "ran offline; tick 'fetch deps' to resolve from crates.io once (the code itself wasn't judged)"),
                                                crate::sandbox::Verdict::ToolError => ("⚠ verify error", theme::AMBER(), ""),
                                            };
                                            ui.horizontal(|ui| {
                                                ui.label(RichText::new(txt).color(col).strong());
                                                if !*sandboxed { ui.label(RichText::new("· unsandboxed").color(theme::FAINT()).small()); }
                                            });
                                            if !note.is_empty() { ui.label(RichText::new(note).color(theme::FAINT()).small()); }
                                            if !output.trim().is_empty() {
                                                egui::ScrollArea::both().id_salt("verify_out").max_height(220.0).show(ui, |ui| {
                                                    ui.add(egui::Label::new(RichText::new(output).monospace().small()));
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                            ui.add_space(6.0);
                            ui.label(RichText::new("Files also appear in the drawer at the bottom (preview, save, history). Verify is the objective grader behind the coordinator loop — the compiler, not an LLM, decides if generated code is real.").color(theme::FAINT()).small());
                        }
                    }
                });
            });
        }

        let mut do_send = false;
        let mut do_continue = false;
        let mut do_checkpoint = false;
        let mut do_export = false;
        let mut do_restore = false;
        let mut rollback: Option<usize> = None;
        let mut raise_ctx: Option<u32> = None;
        let mut action: Option<MsgAction> = None;

        // ── bottom: generated files + composer, PINNED so they never scroll off-screen even
        //    when the Context panel is open or a reply produces a tall file list ──
        egui::Panel::bottom("chat_bottom").resizable(false).show_inside(ui, |ui| {
            ui.add_space(theme::PAD_XS as f32);
            if !arts.is_empty() && self.show_files {
                egui::Frame { fill: theme::PANEL(), corner_radius: theme::r(10), inner_margin: egui::Margin::same(theme::PAD_MD),
                    outer_margin: egui::Margin { bottom: theme::PAD_SM, ..Default::default() }, stroke: egui::Stroke::new(1.0, theme::LINE()), ..Default::default() }
                .show(ui, |ui| {
                    let stitched = arts.iter().filter(|a| a.fragments > 1).count();
                    let hdr = if stitched > 0 {
                        format!("⎙ Generated files ({}) — {stitched} stitched from stop/start · click a row to preview", arts.len())
                    } else {
                        format!("⎙ Generated files ({}) — click a row to preview", arts.len())
                    };
                    // Collapsible drawer (closed by default) so a tall file list never blocks the
                    // chat — the header still shows the count; click to expand. egui skips the body
                    // entirely while collapsed, and remembers the open/closed choice across frames.
                    egui::CollapsingHeader::new(RichText::new(hdr).color(theme::CYAN()).strong().small())
                        .id_salt("gen_files_drawer").default_open(false).show(ui, |ui| {
                    // Provenance view for the expanded file: each prior version linked to the turn (and its
                    // reasoning) that produced it. Precomputed here so the row loop can mutate self freely.
                    let file_hist_view: Vec<(crate::vault::NodeId, String, String)> =
                        match (self.file_hist, self.head, self.vault.as_ref()) {
                            (Some(fi), Some(h), Some(v)) if fi < arts.len() => v.file_history(h, &arts[fi].path).unwrap_or_default()
                                .into_iter().map(|(nid, fe)| {
                                    let why = v.get_text(&fe.produced_by);
                                    let snip = why.lines().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(90).collect::<String>();
                                    (nid, snip, fe.hash.chars().take(8).collect::<String>())
                                }).collect(),
                            _ => Vec::new(),
                        };
                    let mut toggle_hist: Option<usize> = None;
                    egui::ScrollArea::vertical().id_salt("gen_files").auto_shrink([false, true]).max_height(220.0).show(ui, |ui| {
                        for (i, a) in arts.iter().enumerate() {
                            let badge = if a.kind == crate::workspace::Kind::Patch { "patch" } else { "file" };
                            let sz = a.content.len();
                            let szs = if sz >= 1024 { format!("{:.1}K", sz as f32 / 1024.0) } else { format!("{sz} B") };
                            ui.horizontal(|ui| {
                                let open = self.preview == Some(i);
                                let tri = if open { "▾" } else { "▸" };
                                let merged = if a.fragments > 1 { format!("  ·  merged ×{}", a.fragments) } else { String::new() };
                                let mut r = ui.selectable_label(open, RichText::new(format!("{tri} {}  ·  {badge}  ·  {szs}{merged}", a.path)).monospace().small().color(theme::INK()));
                                if a.fragments > 1 { r = r.on_hover_text("Reassembled from pieces emitted across stop/start (warm-resume) — one whole file"); }
                                if r.clicked() { self.preview = if open { None } else { Some(i) }; }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("Copy").on_hover_text("Copy this file's contents to the clipboard").clicked() {
                                        ui.ctx().copy_text(a.content.clone());
                                        self.ws_note = Some((format!("copied {} to clipboard", a.path), Instant::now()));
                                    }
                                    if self.head.is_some() {
                                        let on = self.file_hist == Some(i);
                                        if ui.selectable_label(on, RichText::new("↟").small())
                                            .on_hover_text("Version history — and the reasoning behind each change").clicked() { toggle_hist = Some(i); }
                                    }
                                });
                            });
                            if self.file_hist == Some(i) {
                                egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(6), inner_margin: egui::Margin::same(theme::PAD_SM),
                                    stroke: egui::Stroke::new(1.0, theme::LINE()), ..Default::default() }
                                .show(ui, |ui| {
                                    if file_hist_view.is_empty() {
                                        ui.label(RichText::new("no recorded history for this file yet").color(theme::FAINT()).small());
                                    } else {
                                        ui.label(RichText::new(format!("history of {} — why it changed", a.path)).color(theme::CYAN()).small());
                                        for (nid, snip, vh) in &file_hist_view {
                                            ui.label(RichText::new(format!("#{nid} · v{vh} · {snip}")).color(theme::DIM()).small().monospace());
                                        }
                                    }
                                });
                            }
                            if self.preview == Some(i) {
                                egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(6), inner_margin: egui::Margin::same(theme::PAD_SM), ..Default::default() }
                                .show(ui, |ui| {
                                    egui::ScrollArea::vertical().id_salt(("prev", i)).max_height(160.0).auto_shrink([false, true]).show(ui, |ui| {
                                        ui.label(RichText::new(&a.content).monospace().small().color(theme::DIM()));
                                    });
                                });
                            }
                        }
                    });
                    if let Some(i) = toggle_hist { self.file_hist = if self.file_hist == Some(i) { None } else { Some(i) }; }
                    ui.add_space(theme::PAD_XS as f32);
                    ui.horizontal_wrapped(|ui| {
                        if theme::action_button(ui, "Write workspace", theme::GREEN(), true).clicked() {
                            self.ws_note = Some((match crate::workspace::write_workspace(&arts) {
                                Ok(dir) => { self.last_saved = Some(dir.clone()); format!("wrote {} files → {}", arts.len(), dir.display()) }
                                Err(e) => format!("error: {e}"),
                            }, Instant::now()));
                        }
                        if theme::action_button(ui, "Save .zip", theme::AMBER(), true).clicked() {
                            self.ws_note = Some((match crate::workspace::write_workspace(&arts).and_then(|d| crate::workspace::zip_artifacts(&arts, &d)) {
                                Ok(zip) => { self.last_saved = zip.parent().map(|p| p.to_path_buf()); format!("bundled → {}", zip.display()) }
                                Err(e) => format!("error: {e}"),
                            }, Instant::now()));
                        }
                        if ui.button("📁 Save to folder…").clicked() {
                            let start = self.save_dir.clone()
                                .or_else(|| std::env::var("HOME").ok().map(std::path::PathBuf::from))
                                .unwrap_or_else(|| std::path::PathBuf::from("/"));
                            self.save_picker.open_dir(&start, "Save generated files to…");
                        }
                        if let Some(dir) = self.save_dir.clone() {
                            let name = dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| dir.display().to_string());
                            if ui.button(format!("💾 Save to {name}")).on_hover_text(format!("Re-save into {}", dir.display())).clicked() {
                                self.save_to(&arts, dir);
                            }
                        }
                        if let Some(dir) = self.last_saved.clone() {
                            if ui.button("📂 Open folder").on_hover_text(format!("Reveal {}", dir.display())).clicked() { Self::reveal(&dir); }
                        }
                    });
                    }); // close gen_files_drawer
                });
            }
            if let Some((msg, at)) = &self.ws_note {
                let a = 1.0 - theme::appear(*at, 8.0);
                if a > 0.0 { ui.ctx().request_repaint(); theme::fade(ui, a, |ui| { ui.label(RichText::new(msg).color(theme::GREEN()).small()); }); }
            }
            // context-overflow suggestion: the next prompt won't fit the window, so it'd be trimmed
            if let Some(want) = self.ctx_suggestion(doc) {
                egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(8), inner_margin: egui::Margin::same(theme::PAD_SM),
                    outer_margin: egui::Margin { bottom: theme::PAD_XS, ..Default::default() }, stroke: egui::Stroke::new(1.0, theme::AMBER()), ..Default::default() }
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("⚠").color(theme::AMBER()).small());
                        ui.label(RichText::new(format!("This is longer than the {}-token window — it'll be trimmed.", self.ctx.n_ctx)).color(theme::DIM()).small());
                        if theme::action_button(ui, &format!("Raise to {want}"), theme::AMBER(), true).clicked() { raise_ctx = Some(want); }
                    });
                });
            }
            if self.show_checkpoints && !self.checkpoints.is_empty() {
                egui::Frame { fill: theme::PANEL(), corner_radius: theme::r(8), inner_margin: egui::Margin::same(theme::PAD_SM),
                    outer_margin: egui::Margin { bottom: theme::PAD_XS, ..Default::default() }, stroke: egui::Stroke::new(1.0, theme::LINE()), ..Default::default() }
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("ckpts").max_height(140.0).show(ui, |ui| {
                        for (i, cp) in self.checkpoints.iter().enumerate().rev() {
                            ui.horizontal(|ui| {
                                if theme::action_button(ui, "Restore", theme::CYAN(), !streaming).clicked() { rollback = Some(i); }
                                ui.label(RichText::new(format!("{} · {} msgs", cp.label, cp.msgs.len())).color(theme::DIM()).small());
                            });
                        }
                    });
                });
            }
            // history tree: every committed turn is a node; "Return" loads it and the next turn forks.
            if self.show_history {
                let head_now = self.head;
                let rows: Vec<(crate::vault::NodeId, Option<crate::vault::NodeId>, String, usize, usize)> =
                    self.vault.as_ref().map(|v| v.all_ids().iter().rev()
                        .filter_map(|&id| v.node(id).ok().map(|n| (n.id, n.parent, n.label.clone(), n.files.len(), n.cum_len)))
                        .collect()).unwrap_or_default();
                let mut return_to: Option<crate::vault::NodeId> = None;
                if rows.is_empty() {
                    ui.label(RichText::new("no history yet — finish a turn to create the first node").color(theme::FAINT()).small());
                } else {
                    egui::Frame { fill: theme::PANEL(), corner_radius: theme::r(8), inner_margin: egui::Margin::same(theme::PAD_SM),
                        outer_margin: egui::Margin { bottom: theme::PAD_XS, ..Default::default() }, stroke: egui::Stroke::new(1.0, theme::LINE()), ..Default::default() }
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().id_salt("tree").max_height(170.0).show(ui, |ui| {
                            for (id, parent, label, nfiles, msgs) in &rows {
                                ui.horizontal(|ui| {
                                    let here = head_now == Some(*id);
                                    if theme::action_button(ui, if here { "● here" } else { "Return" }, theme::CYAN(), !streaming && !here).clicked() { return_to = Some(*id); }
                                    let from = parent.map(|p| format!(" ←#{p}")).unwrap_or_else(|| " (root)".into());
                                    ui.label(RichText::new(format!("#{id}{from}  {}  · {} msgs · {} files", label, msgs, nfiles)).color(if here { theme::CYAN() } else { theme::DIM() }).small());
                                });
                            }
                        });
                    });
                }
                if let Some(id) = return_to { self.return_to_node(id); }
            }
            // suggestion chips (starters / follow-ups) — click to fill the composer
            if self.input.trim().is_empty() {
                let sugg = self.suggestions(doc);
                if !sugg.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for s in &sugg {
                            if theme::carved_button_tinted(ui, s, theme::accent()).clicked() {
                                self.input = s.clone();
                            }
                        }
                    });
                    ui.add_space(theme::PAD_XS as f32);
                }
            }
            // id autocomplete — typing a node-id prefix in :mergectx / :goto offers matching ids; click to complete
            else {
                // command palette: a hint line for the command being typed (persists through its arguments),
                // and a zsh-style grid of name completions while the command itself is still being typed.
                if let Some(h) = crate::vimmode::command_hint(&self.input) {
                    let args = if h.args.is_empty() { String::new() } else { format!(" {}", h.args) };
                    ui.label(RichText::new(format!(":{}{}  —  {}", h.name, args, h.help)).color(theme::AMBER_INK()).small());
                }
                let cmds = crate::vimmode::command_completions(&self.input);
                if !cmds.is_empty() {
                    egui::Grid::new("cmd_palette").num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
                        for (k, c) in cmds.iter().enumerate() {
                            let hover = if c.aliases.is_empty() { format!(":{} {}\n{}", c.name, c.args, c.help) }
                                        else { format!(":{} {}\naliases: {}\n{}", c.name, c.args, c.aliases.join(", "), c.help) };
                            if ui.add(egui::Button::new(RichText::new(format!(":{}", c.name)).monospace().color(theme::CYAN()).small()).frame(true))
                                .on_hover_text(hover).clicked() {
                                self.input = format!(":{} ", c.name);  // complete the command, ready for its arguments
                            }
                            if k % 4 == 3 { ui.end_row(); }
                        }
                    });
                    ui.add_space(theme::PAD_XS as f32);
                } else {
                    let comps = self.id_completions();
                    if !comps.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("ids").color(theme::DIM()).small());
                            for (id, completed) in &comps {
                                if ui.add(egui::Button::new(RichText::new(id).monospace().color(theme::VIOLET()).small()).frame(true)).clicked() {
                                    self.input = completed.clone();
                                }
                            }
                        });
                        ui.add_space(theme::PAD_XS as f32);
                    }
                }
            }
            let composer_frame = egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(10), inner_margin: egui::Margin::same(theme::PAD_MD),
                outer_margin: egui::Margin { bottom: theme::PAD_SM, ..Default::default() }, stroke: egui::Stroke::new(1.0, theme::LINE()), ..Default::default() }
            .show(ui, |ui| {
                if let Some(bf) = self.branch_from {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("⑂ branching from turn {} — your next message forks a side-thread", bf + 1)).color(egui::Color32::from_rgb(0xc9, 0x8f, 0xff)).small().strong());
                        if ui.small_button("✕ cancel").clicked() { self.branch_from = None; }
                    });
                    ui.add_space(2.0);
                }
                // Stable, explicit id: the suggestion chips above are rendered only while the input
                // is empty (see above), so typing the first character removes those widgets and would
                // shift this TextEdit's AUTO id — which makes egui drop focus after one keystroke.
                // Pinning the id keeps focus across that empty→non-empty layout change.
                // The composer is a PEARL: the typed text is drawn as temporally-stable resonance cells (same
                // language as the 3D pearls), with a real TextEdit overlaid transparently on the same band so it
                // still owns input, caret, selection and IME — and the mobile focus fix below still applies.
                let pearl_t = ui.input(|i| i.time) as f32;
                let band = self.pearl_input.show(ui, &self.input, pearl_t, 132.0, theme::CYAN());
                // the pearl draws its OWN caret shimmer aligned to the cells; hide egui's caret (it sits at the
                // transparent TextEdit's content origin — up/left of the cells) and its selection fill.
                ui.visuals_mut().text_cursor.stroke = egui::Stroke::NONE;
                ui.visuals_mut().selection.bg_fill = egui::Color32::from_rgba_unmultiplied(0x4d, 0xd6, 0xe6, 36);
                let resp = ui.put(band, egui::TextEdit::multiline(&mut self.input)
                    .id(egui::Id::new("logscope_chat_composer"))
                    .desired_width(f32::INFINITY)
                    .frame(egui::Frame::NONE)
                    .text_color(egui::Color32::TRANSPARENT)   // invisible text — the pearl cells are the visual
                    .font(egui::FontId::new(14.0, egui::FontFamily::Monospace)));
                let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                if enter && self.input.ends_with('\n') { self.input.pop(); }
                // Plasma Mobile / IME robustness: on some mobile stacks (Wayland + Maliit) the soft-keyboard
                // commit drops widget focus right after the FIRST character — the empty→non-empty layout change
                // re-targets the IME, so only one char lands. Signature: the text changed THIS frame but focus
                // was lost in the SAME frame. When we see that, re-assert focus so typing continues. This is
                // inert on desktop (focus is retained there) and when the user navigates away (no text change).
                if resp.changed() {
                    self.mode = crate::vimmode::Mode::Insert;
                    if !resp.has_focus() { resp.request_focus(); }
                }
                ui.add_space(theme::PAD_XS as f32);
                ui.horizontal(|ui| {
                    if theme::carved_button(ui, "📎 Attach").on_hover_text("Attach a text file into your message (bounded)").clicked() {
                        let start = self.save_dir.clone()
                            .or_else(|| std::env::var("HOME").ok().map(std::path::PathBuf::from))
                            .unwrap_or_else(|| std::path::PathBuf::from("/"));
                        self.attach_picker.open_file(&start, "Attach a file", &[]);
                    }
                    ui.label(RichText::new(if self.threading { "threaded" } else { "single-shot" }).color(theme::FAINT()).small());
                    ui.checkbox(&mut self.auto_continue, RichText::new("auto-continue").small())
                        .on_hover_text("When a reply hits the length cap, keep generating to finish it");
                    if self.hit_cap && !streaming {
                        if theme::action_button(ui, "⏭ Continue", theme::CYAN(), true)
                            .on_hover_text("The last reply hit the length cap — resume it").clicked() { do_continue = true; }
                    }
                    if !arts.is_empty() {
                        let n = arts.len();
                        if ui.selectable_label(self.show_files, RichText::new(format!("⎙ {n}")).small())
                            .on_hover_text("Show/hide the generated files from the latest reply").clicked() { self.show_files = !self.show_files; }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if streaming {
                            theme::spinner(ui, 16.0, theme::AMBER());
                            if theme::action_button(ui, "⏹ Stop", theme::RED(), true).clicked() { infer.cancel(); }
                            let can = !self.input.trim().is_empty();
                            if theme::action_button(ui, "↪ Queue", theme::CYAN(), can)
                                .on_hover_text("Fire this as a branch when the current reply finishes — no need to stop the model").clicked() { do_send = true; }
                            if !self.queue.is_empty() { ui.label(RichText::new(format!("{} queued", self.queue.len())).color(theme::CYAN()).small()); }
                        } else {
                            let can = !self.input.trim().is_empty();
                            if theme::action_button(ui, "Send ▶", theme::AMBER(), can).clicked() { do_send = true; }
                            if !self.queue.is_empty() { ui.label(RichText::new(format!("{} queued", self.queue.len())).color(theme::CYAN()).small()); }
                        }
                        // collapsed in from the old top bar: checkpoint · history · tree · export
                        ui.add_space(10.0);
                        if ui.add_enabled(!self.msgs.is_empty(), egui::Button::new(RichText::new("⧉").small()))
                            .on_hover_text("Export the chat as Markdown").clicked() { do_export = true; }
                        if self.vault.is_some() {
                            let n = self.vault.as_ref().map(|v| v.all_ids().len()).unwrap_or(0);
                            if ui.button(RichText::new(format!("🌲 {n}")).small())
                                .on_hover_text("Project history tree — return to any point to branch a new line").clicked() { self.show_history = !self.show_history; }
                        }
                        if !self.checkpoints.is_empty() {
                            if ui.button(RichText::new(format!("🕘 {}", self.checkpoints.len())).small())
                                .on_hover_text("Checkpoint history (rollback)").clicked() { self.show_checkpoints = !self.show_checkpoints; }
                        }
                        if ui.add_enabled(!self.msgs.is_empty(), egui::Button::new(RichText::new("📌").small()))
                            .on_hover_text("Save a restore point for this conversation").clicked() { do_checkpoint = true; }
                    });
                });
                if enter { do_send = true; self.pearl_input.pulse(); } // send() queues it if the model is busy
            });
            theme::carved_trough(ui.painter(), composer_frame.response.rect, ui.input(|i| i.time) as f32);
        });

        // ── center: the message log fills whatever height remains above the pinned bottom ──
        egui::CentralPanel::default().show_inside(ui, |ui| {
            if self.spatial_view { self.spatial_ui(ui); return; } // aggressive: spatial graph replaces the linear log
            egui::ScrollArea::vertical().id_salt("chat_log").auto_shrink([false, false]).stick_to_bottom(true)
                .show(ui, |ui| {
                    if let Some(n) = self.recover {
                        egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(10), inner_margin: egui::Margin::same(theme::PAD_MD),
                            outer_margin: egui::Margin { bottom: theme::PAD_SM, ..Default::default() }, stroke: egui::Stroke::new(1.0, theme::CYAN()), ..Default::default() }
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new("⟲").color(theme::CYAN()).strong());
                                ui.label(RichText::new(format!("A previous conversation ({n} messages) was found.")).color(theme::INK()));
                                if theme::action_button(ui, "Recover", theme::CYAN(), true).clicked() { do_restore = true; }
                                if ui.button("Dismiss").clicked() { self.recover = None; }
                            });
                        });
                    }
                    if self.msgs.is_empty() {
                        ui.add_space(16.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("No messages yet").color(theme::DIM()));
                            ui.label(RichText::new("Type below to start. Toggle Thread for multi-turn, or attach a file with 📎.").color(theme::FAINT()).small());
                        });
                    }
                    let cur = self.stream.as_ref().map(|s| s.idx);
                    let streaming_any = cur.is_some();
                    let nmsgs = self.msgs.len();
                    let scroll_to = self.scroll_chat_to.take(); // jump-scroll target from the graph
                    for (i, m) in self.msgs.iter_mut().enumerate() {
                        if Some(i) == scroll_to { ui.scroll_to_cursor(Some(egui::Align::Center)); }
                        let (who, c, fill) = match m.role {
                            Role::User => ("you", theme::AMBER(), theme::PANEL2()),
                            Role::Assistant => ("assistant", theme::GREEN(), theme::PANEL()),
                        };
                        let a = theme::appear(m.born, 0.18);
                        if a < 1.0 { ui.ctx().request_repaint(); }
                        let streaming_here = cur == Some(i);
                        theme::fade(ui, a, |ui| {
                            egui::Frame { fill, corner_radius: theme::r(10),
                                inner_margin: egui::Margin::symmetric(theme::PAD_MD, theme::PAD_SM + 2),
                                outer_margin: egui::Margin { bottom: theme::PAD_SM, ..Default::default() },
                                stroke: egui::Stroke::new(1.0, theme::LINE()), ..Default::default() }
                            .show(ui, |ui| {
                                // header: role · branch pager · controls
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(who).color(c).small().strong());
                                    // node id — highlighted so it can be referenced in :mergectx (git-style prefix)
                                    ui.label(RichText::new(crate::vimmode::short_id(i)).monospace().small().color(theme::VIOLET()))
                                        .on_hover_text("node id — reference it in :mergectx (a unique prefix works, e.g. :mergectx a9j,3df)");
                                    if m.variants.len() > 1 {
                                        if ui.small_button("‹").on_hover_text("Previous branch").clicked() { action = Some(MsgAction::PrevVar(i)); }
                                        ui.label(RichText::new(format!("{}/{}", m.cur + 1, m.variants.len())).color(theme::FAINT()).small());
                                        if ui.small_button("›").on_hover_text("Next branch").clicked() { action = Some(MsgAction::NextVar(i)); }
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if !streaming_any && !m.editing {
                                            if ui.small_button("🗑").on_hover_text("Delete from here down").clicked() { action = Some(MsgAction::Delete(i)); }
                                            if !m.text().is_empty() && ui.small_button("⧉").on_hover_text("Copy message").clicked() { action = Some(MsgAction::Copy(i)); }
                                            if m.role == Role::User && ui.small_button("✎").on_hover_text("Edit & re-run from here").clicked() { action = Some(MsgAction::EditStart(i)); }
                                            if m.role == Role::Assistant && i + 1 >= nmsgs && ui.small_button("↻").on_hover_text("Reroll (keeps the old as a branch)").clicked() { action = Some(MsgAction::Regenerate(i)); }
                                        }
                                    });
                                });
                                ui.add_space(2.0);
                                if m.editing {
                                    ui.add(egui::TextEdit::multiline(&mut m.edit_buf).id(egui::Id::new(("logscope_chat_edit", i))).desired_rows(2).desired_width(f32::INFINITY));
                                    ui.horizontal(|ui| {
                                        if theme::action_button(ui, "Save & re-run", theme::GREEN(), true).clicked() { action = Some(MsgAction::EditSave(i)); }
                                        if ui.button("Cancel").clicked() { action = Some(MsgAction::EditCancel(i)); }
                                    });
                                } else {
                                    let segs = segment_message(m.text());
                                    let last = segs.len().saturating_sub(1);
                                    let think_open = m.think_open;
                                    let mut toggle_think = false;
                                    for (si, seg) in segs.iter().enumerate() {
                                        let tail = if streaming_here && si == last { "▌" } else { "" };
                                        match seg {
                                            Seg::Prose(p) => { render_markdown(ui, p, tail); }
                                            Seg::Think(t) => {
                                                let open = ui.ctx().animate_bool(egui::Id::new((i, si, "think")), think_open);
                                                let arrow = if think_open { "▾" } else { "▸" };
                                                if ui.add(egui::Button::new(RichText::new(format!("{arrow} thinking")).color(theme::FAINT()).small().italics()).frame(false)).clicked() { toggle_think = true; }
                                                if open > 0.0 {
                                                    theme::fade(ui, open, |ui| {
                                                        egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(6), inner_margin: egui::Margin::same(theme::PAD_SM),
                                                            stroke: egui::Stroke::new(1.0, theme::LINE()), ..Default::default() }
                                                        .show(ui, |ui| { ui.label(RichText::new(format!("{t}{tail}")).color(theme::DIM()).italics()); });
                                                    });
                                                }
                                            }
                                            Seg::Code { lang, body } => {
                                                egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(6),
                                                    inner_margin: egui::Margin::same(theme::PAD_SM), stroke: egui::Stroke::new(1.0, theme::LINE()), ..Default::default() }
                                                .show(ui, |ui| {
                                                    ui.horizontal(|ui| {
                                                        let l = lang.split_whitespace().next().unwrap_or("");
                                                        if !l.is_empty() { ui.label(RichText::new(l).color(theme::CYAN()).small().monospace()); }
                                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                            if ui.small_button("Copy").on_hover_text("Copy this block").clicked() { ui.ctx().copy_text(body.clone()); }
                                                        });
                                                    });
                                                    ui.label(RichText::new(format!("{body}{tail}")).monospace().color(theme::INK()));
                                                });
                                            }
                                        }
                                    }
                                    if toggle_think { m.think_open = !m.think_open; }
                                    if m.text().is_empty() && streaming_here { ui.label(RichText::new("▌").color(theme::INK())); }
                                }
                            });
                        });
                    }
                });
        });

        // ── picker modals + their outcomes ──
        if let Some(crate::filepicker::Outcome::Picked(dir)) = self.save_picker.ui(ui.ctx()) {
            self.save_to(&arts, dir);
        }
        if let Some(crate::filepicker::Outcome::Picked(file)) = self.attach_picker.ui(ui.ctx()) {
            self.attach(&file);
        }
        if let Some(act) = action {
            let ctx = ui.ctx().clone();
            self.apply_msg_action(&ctx, act, infer, doc);
        }
        if do_send { self.send(infer, doc); }
        // Drain one queued (branch) turn as soon as the model is free — serial, but non-blocking from
        // the user's side: they fired it mid-generation and it runs the moment the current reply ends.
        if self.stream.is_none() {
            // Between rounds of a depth fan: once this round's generations have all finished and drained,
            // fire the next round on the NEW frontier (this is what makes :Nca / :Nfa go N deep).
            if self.queue.is_empty() && self.fan.is_some() { self.fan_step(); }
            if let Some((qt, parent)) = self.queue.pop_front() {
                if qt == CONTINUE_ALL_TOK {
                    if let Some(l) = parent { self.expand_leaf(l, infer, doc); }
                } else {
                    self.start_generation(qt, parent, infer, doc);
                }
                ui.ctx().request_repaint();
            }
        }
        if do_continue { self.continue_generation(infer, doc); }
        if do_checkpoint { self.checkpoint(format!("manual · {} msgs", self.msgs.len())); }
        if do_export {
            let note = match self.export_chat(ui.ctx()) {
                Some(p) => format!("Copied to clipboard · saved {}", p.display()),
                None => "Copied chat to clipboard".to_string(),
            };
            self.ws_note = Some((note, Instant::now()));
        }
        if do_restore { self.restore_autosave(); }
        if let Some(i) = rollback { self.rollback_to(i); }
        if let Some(want) = raise_ctx { self.ctx.n_ctx = want.min(32768); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Doc;

    #[test]
    fn color_salience_rises_with_use_and_is_zero_without() {
        // no colour words → 0; the rug stays a neutral mat
        assert_eq!(chat_color_salience(["just talking about code and systems here"].into_iter()), 0.0);
        // mentions → positive, and MORE mentions (raw + term-frequency) → strictly higher
        let one = chat_color_salience(["the colour of the sky"].into_iter());
        let many = chat_color_salience(["colour colour colour palette hue and more colour talk"].into_iter());
        assert!(one > 0.0, "a mention registers");
        assert!(many > one, "salience rises as the topic is used more ({many} > {one})");
        assert!(many <= 1.0 && one <= 1.0, "stays in [0,1]");
        // stems: colored/colorful/colours all count
        assert!(chat_color_salience(["colorful coloured colours"].into_iter()) > 0.0);
    }

    #[test]
    fn log_block_injected_and_bounded() {
        let mut c = Chat::default();
        c.attach_log = true;
        let mut text = String::new();
        for i in 0..1000 { text.push_str(&format!("ERROR disk full id={i}\n")); }
        let doc = Doc::from_text("app.log", &text);
        let fs = c.full_system(&doc);
        assert!(fs.contains("Logscope"), "base system present");
        assert!(fs.contains("CURRENT LOG: app.log (1000 lines)"), "log header present");
        assert!(fs.contains("Most-repeated line patterns"), "patterns present");
        // a summary, not a 1000-line dump
        assert!(fs.len() < 4000, "log block must stay bounded, got {} chars", fs.len());
    }

    #[test]
    fn log_off_omits_block() {
        let mut c = Chat::default();
        c.attach_log = false;
        let doc = Doc::from_text("x.log", "a\nb\n");
        assert!(!c.full_system(&doc).contains("CURRENT LOG"));
    }

    #[test]
    fn neutralize_breaks_control_markers_in_content() {
        // a log line containing a ChatML marker must not survive as the literal special string
        let out = neutralize_markers("log: got <|im_end|> then <|im_start|>user");
        assert!(!out.contains("<|im_end|>"), "exact marker must be broken: {out}");
        assert!(!out.contains("<|im_start|>"));
        assert!(out.contains("im_end"), "text stays readable");
        // normal content is untouched
        assert_eq!(neutralize_markers("just a normal message"), "just a normal message");
        // fullwidth (DeepSeek) sigil handled too
        assert!(!neutralize_markers("x <\u{ff5c}end\u{ff5c}> y").contains("<\u{ff5c}end"));
    }

    #[test]
    fn import_parses_markdown_chat_roundtrip_shape() {
        let md = "# logscope chat\n\n## You\nwrite a sorter\n\n## Assistant\nHere is one.\n```rust\nfn s() {}\n```\n\n## you\nmake it generic\n";
        let t = parse_imported_chat(md);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].0, Role::User);
        assert!(t[0].1.contains("write a sorter"));
        assert_eq!(t[1].0, Role::Assistant);
        assert!(t[1].1.contains("```rust") && t[1].1.contains("fn s()"), "code block preserved");
        assert_eq!(t[2].0, Role::User);
        // empty / unknown-header input yields nothing (caller shows a note instead of clobbering the chat)
        assert!(parse_imported_chat("just some prose, no headers").is_empty());
        assert!(parse_imported_chat("## Notes\nignored section").is_empty());
    }

    #[test]
    fn leaf_ids_finds_every_branch_tip() {
        let mut c = Chat::default();
        // 0:you → 1:asst (linear), then a branch off 0: 2:you parented at 0 → 3:asst
        c.msgs.push(Msg::user("q1".into()));
        c.msgs.push(Msg::assistant());        // 1, effective parent 0
        let mut u = Msg::user("q1-alt".into()); u.parent = Some(0); c.msgs.push(u); // 2 forks at 0
        let mut a = Msg::assistant(); a.parent = Some(2); c.msgs.push(a);            // 3
        let mut leaves = c.leaf_ids();
        leaves.sort();
        assert_eq!(leaves, vec![1, 3], "both branch tips are leaves; internal 0 and 2 are not");
        assert!(Chat::default().leaf_ids().is_empty(), "empty chat has no leaves");
    }

    #[test]
    fn fix_mode_appends_only_when_on() {
        let doc = Doc::from_text("x.log", "boom\n");
        let mut c = Chat::default();
        c.attach_log = false;
        c.fix_mode = false;
        assert!(!c.full_system(&doc).contains("FIX MODE"), "off → no fix block");
        c.fix_mode = true;
        let fs = c.full_system(&doc);
        assert!(fs.contains("Logscope"), "base system still present");
        assert!(fs.contains("FIX MODE is on"), "on → fix block appended");
    }

    #[test]
    fn segments_prose_and_code() {
        let segs = segment_message("intro line\n```rust src/x.rs\nfn x() {}\n```\noutro");
        assert_eq!(segs.len(), 3);
        assert!(matches!(&segs[0], Seg::Prose(p) if p == "intro line"));
        match &segs[1] { Seg::Code { lang, body } => { assert!(lang.starts_with("rust")); assert_eq!(body, "fn x() {}"); }, _ => panic!("expected code") }
        assert!(matches!(&segs[2], Seg::Prose(p) if p == "outro"));
    }

    #[test]
    fn segments_unclosed_fence_streams_as_code() {
        // mid-stream: a fence is open but not yet closed — the partial body renders as code
        let segs = segment_message("here:\n```py\nprint(1)");
        assert_eq!(segs.len(), 2);
        assert!(matches!(&segs[0], Seg::Prose(_)));
        match &segs[1] { Seg::Code { lang, body } => { assert_eq!(lang, "py"); assert_eq!(body, "print(1)"); }, _ => panic!("expected code") }
    }

    #[test]
    fn segments_plain_text() {
        let segs = segment_message("no code here at all");
        assert_eq!(segs.len(), 1);
        assert!(matches!(&segs[0], Seg::Prose(p) if p == "no code here at all"));
    }

    #[test]
    fn checkpoint_save_roundtrips_through_json() {
        // a conversation with an assistant turn that has two branches (cur=1)
        let msgs = vec![
            Msg::user("why is the disk full?".into()),
            Msg { role: Role::Assistant, variants: vec!["first try".into(), "better answer with ```code```".into()], cur: 1,
                  born: Instant::now(), editing: false, edit_buf: String::new(), think_open: true, parent: None },
        ];
        let save = ChatSave {
            msgs: msgs.iter().map(msg_to_save).collect(),
            checkpoints: vec![Checkpoint { label: "before: x".into(), at_unix: 42, msgs: msgs.iter().map(msg_to_save).collect() }],
        };
        let json = serde_json::to_string(&save).expect("serialize");
        let back: ChatSave = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.msgs.len(), 2);
        assert_eq!(back.checkpoints.len(), 1);
        // restore the assistant turn: role, both variants, and the selected branch survive
        let restored: Vec<Msg> = back.msgs.iter().map(save_to_msg).collect();
        assert_eq!(restored[0].role, Role::User);
        assert_eq!(restored[0].text(), "why is the disk full?");
        assert_eq!(restored[1].variants.len(), 2);
        assert_eq!(restored[1].cur, 1);
        assert_eq!(restored[1].text(), "better answer with ```code```");
    }

    #[test]
    fn save_to_msg_clamps_a_bad_cur() {
        // a corrupt/old save with cur past the end must not panic — clamp into range
        let m = save_to_msg(&MsgSave { role: 1, variants: vec!["only".into()], cur: 9 });
        assert_eq!(m.cur, 0);
        assert_eq!(m.text(), "only");
        // empty variants must yield a usable single empty variant
        let e = save_to_msg(&MsgSave { role: 0, variants: vec![], cur: 3 });
        assert_eq!(e.variants.len(), 1);
        assert_eq!(e.cur, 0);
    }

    #[test]
    fn segments_thinking_block() {
        let segs = segment_message("<think>let me reason\nstep two</think>The answer is 42.");
        assert_eq!(segs.len(), 2);
        assert!(matches!(&segs[0], Seg::Think(t) if t.contains("step two")));
        assert!(matches!(&segs[1], Seg::Prose(p) if p == "The answer is 42."));
        // unclosed think (mid-stream) renders as thinking
        let s2 = segment_message("intro\n<think>still reasoning");
        assert!(matches!(s2.last().unwrap(), Seg::Think(t) if t == "still reasoning"));
    }

    #[test]
    fn plain_prompt_shape() {
        let mut c = Chat::default();
        c.backend = BackendKind::LlamaCpp;
        c.attach_log = false;
        c.msgs.push(Msg::user("why is the disk full?".into()));
        let p = c.build_prompt(&Doc::from_text("", ""));
        assert!(p.starts_with("System: "));
        assert!(p.contains("why is the disk full?"));
        assert!(p.trim_end().ends_with("Assistant:"));
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
        "Atlantis"     => (c(0x6a, 0xc6, 0xd2), c(0x10, 0x2a, 0x36), 0.62, 0.98), // aqua, cool floor bounce
        "Canopy"       => (c(0x9a, 0xb2, 0x6e), c(0x22, 0x30, 0x1c), 0.56, 0.88), // dim mossy library green
        "Mistfall"     => (c(0xb2, 0xc6, 0xd4), c(0x20, 0x2a, 0x30), 0.62, 0.95), // diffuse misty grey-blue
        "Archive"      => (c(0xe8, 0xc2, 0x80), c(0x2a, 0x20, 0x14), 0.56, 0.95), // dim warm amber (wood)
        "Reading Room" => (c(0xe8, 0xe4, 0xd6), c(0x90, 0x86, 0x72), 0.60, 0.82), // airy paper — toned down (was blowing whites)
        "Folding@home" => (c(0x9a, 0xb0, 0xd0), c(0x16, 0x1c, 0x26), 0.70, 1.00), // cool clinical
        "Cloudscape"   => (c(0x4e, 0x5e, 0x82), c(0x20, 0x28, 0x3a), 0.50, 0.60), // MOONLIT night sky — cool, dim (was reading like daylight)
        "Forest"       => (c(0xd0, 0xe2, 0x8a), c(0x2e, 0x3c, 0x1e), 0.86, 1.06), // sunlit woodland green
        "Treehouse"    => (c(0xe2, 0xb0, 0x6e), c(0x1c, 0x26, 0x12), 0.58, 1.0),  // warm dusk canopy (fireflies pop)
        "Mansion Garden" => (c(0xd2, 0xc8, 0xb0), c(0x76, 0x6e, 0x58), 0.54, 0.70), // cream marble — toned further (still too bloomy)
        _              => (c(140, 168, 205),    c(96, 82, 66),       0.74, 1.00),
    }
}
