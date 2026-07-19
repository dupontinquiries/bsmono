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

/// The mic pearl's SHAPE — a SECOND per-scene derivative (beyond the colour idiom) so the in-scene voice
/// indicator matches each world's physical substance, not just its tint. Kept a FREE derivative (no bespoke
/// per-world table): it falls out of the palette `Material` plus a small crystalline/watery name set —
///   * `Pearl`  — smooth orb: watery/airy worlds (Atlantis, Cloudscape) and paper worlds (a wafer).
///   * `Gem`    — faceted iridescent crystal: the crystalline worlds (Fractal Spires, Recursive Citadel,
///                Frozen Bifrost) — the pass-67 "faceted gem vs smooth pearl" second axis.
///   * `Stone`  — weathered/carved stone bead: rock & marble & cave/mist worlds (Slate, Titan's Ribcage,
///                Mistfall, Mansion Garden…).
///   * `Bead`   — chunky turned wooden bead: the wood-board worlds (Old Library, Archive, Alley…).
///   * `Leaf`   — a backlit leaf blade: the foliage worlds (Forest, Canopy, Treehouse, Hollow Jungle) — the
///                signature "leaves instead of the pearl" object.
/// Rendering maps these onto existing mesh primitives (uv_sphere / cracked_pearl / rock / leaf_single).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MicShape { Pearl, Gem, Stone, Bead, Leaf }

/// Derive the mic pearl shape for a scene from its name + palette material (see `MicShape`). Pure so it can
/// be unit-tested without a renderer (the GUI verification path is headless render; this is the logic gate).
pub fn mic_shape(scene: &str, material: theme::Material) -> MicShape {
    use theme::Material::*;
    match scene {
        // foliage worlds → a backlit LEAF blade (the signature "leaves instead of the pearl" object).
        "Forest" | "Canopy" | "Treehouse" | "Hollow Jungle" => MicShape::Leaf,
        // crystalline worlds → a faceted gem (the shape derivative the pass-67 note calls for).
        "Fractal Spires" | "Recursive Citadel" | "Frozen Bifrost" => MicShape::Gem,
        // cave / carved-rune / mist worlds → weathered stone (the "caves → stones" idiom).
        "Mistfall" => MicShape::Stone,
        // watery / airy worlds → a smooth pearl regardless of their (stone) material.
        "Atlantis" | "Cloudscape" => MicShape::Pearl,
        // everything else follows its physical material.
        _ => match material {
            Wood => MicShape::Bead,          // a turned wooden bead
            Paper | None => MicShape::Pearl, // a paper wafer reads as a smooth bead
            Stone | Marble => MicShape::Stone,
        },
    }
}

/// Project a billboarded in-scene UI slab (centre + right/up basis, size w×h in world units) to its
/// screen-space bounding Rect. This is the basis for making in-scene panels/buttons INTERACTIVE and
/// ACCESSIBLE: the 3D slab is the visual, and an egui widget placed over this rect is the hit-test +
/// AccessKit overlay, so pointer clicks and screen-reader focus both target the physical object in the
/// scene. Returns None if any corner is behind the camera (don't interact with an off-screen slab).
pub fn project_slab_rect(cam: &crate::spatial::OrbitCam, center: crate::spatial::Vec3,
                         right: crate::spatial::Vec3, up: crate::spatial::Vec3, w: f32, h: f32,
                         cx: f32, cy: f32, scale: f32) -> Option<egui::Rect> {
    let (hw, hh) = (w * 0.5, h * 0.5);
    let corner = |sx: f32, sy: f32| crate::spatial::Vec3::new(
        center.x + right.x * (sx * hw) + up.x * (sy * hh),
        center.y + right.y * (sx * hw) + up.y * (sy * hh),
        center.z + right.z * (sx * hw) + up.z * (sy * hh));
    let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let (x, y, _d) = cam.project(corner(sx, sy), cx, cy, scale)?;
        lo.0 = lo.0.min(x); lo.1 = lo.1.min(y); hi.0 = hi.0.max(x); hi.1 = hi.1.max(y);
    }
    Some(egui::Rect::from_min_max(egui::pos2(lo.0, lo.1), egui::pos2(hi.0, hi.1)))
}

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

/// Read a `Ctx` field as an f32 by its physical-dial id (for syncing the dial handle to the live value).
fn ctx_get(ctx: &Ctx, id: usize) -> f32 {
    match id {
        PARAM_TEMP => ctx.temperature,
        PARAM_NCTX => ctx.n_ctx as f32,
        PARAM_MAXTOK => ctx.max_tokens as f32,
        PARAM_REPEAT => ctx.repeat_penalty,
        PARAM_MINP => ctx.min_p,
        _ => 0.0,
    }
}
/// Write a `Ctx` field from a dragged dial value (rounding the integer params).
fn ctx_set(ctx: &mut Ctx, id: usize, v: f32) {
    match id {
        PARAM_TEMP => ctx.temperature = v,
        PARAM_NCTX => ctx.n_ctx = v.round() as u32,
        PARAM_MAXTOK => ctx.max_tokens = v.round() as u32,
        PARAM_REPEAT => ctx.repeat_penalty = v,
        PARAM_MINP => ctx.min_p = v,
        _ => {}
    }
}

/// Build the in-scene control dock: every scene control is a physical object — Overview/Prune/Unprune
/// momentary, Tour/Freecam toggles — plus the Context panel's sampling sliders as physical Dials.
fn phys_dock() -> crate::physui::Panel {
    use crate::physui::{ControlKind, Panel};
    let mut p = Panel::new();
    p.add(DOCK_OVERVIEW, ControlKind::Momentary, false);
    p.add(DOCK_TOUR, ControlKind::Toggle, false);
    p.add(DOCK_FREECAM, ControlKind::Toggle, false);
    p.add(DOCK_PRUNE, ControlKind::Momentary, false);
    p.add(DOCK_UNPRUNE, ControlKind::Momentary, false);
    let cd = Ctx::default();
    for &(id, _, lo, hi, _) in &PARAM_DIALS { p.add_dial(id, lo, hi, ctx_get(&cd, id)); }
    p.add_selector(BACKEND_SEL, BackendKind::ALL.len(), 0);
    p.add_selector(PERSONA_SEL, PERSONAS.len(), 0);
    p.add_selector(MODEL_SEL, 1, 0); // real count set each frame via set_selector_count once known
    p
}

/// The camera/context change a physical-dock actuation implies. Pure + testable so the button→action
/// mapping (and the fact that non-dock events are ignored) is locked without a live scene; the field
/// mutations that apply it mirror the controls the (now-removed) 2D toolbar used to expose.
#[derive(Clone, Copy, PartialEq, Debug)]
enum DockAction { Overview, Tour(bool), Freecam(bool), Prune, Unprune }

/// Map a [`physui::ControlEvent`] from the dock to its [`DockAction`], or `None` for events that aren't
/// one of the dock's controls (unknown ids, dial adjusts, etc.).
fn dock_action(ev: crate::physui::ControlEvent) -> Option<DockAction> {
    use crate::physui::ControlEvent::*;
    match ev {
        Fired(DOCK_OVERVIEW) => Some(DockAction::Overview),
        Toggled(DOCK_TOUR, on) => Some(DockAction::Tour(on)),
        Toggled(DOCK_FREECAM, on) => Some(DockAction::Freecam(on)),
        Fired(DOCK_PRUNE) => Some(DockAction::Prune),
        Fired(DOCK_UNPRUNE) => Some(DockAction::Unprune),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Role { User, Assistant }

/// A specular GLINT for a camera-facing physical slab: a soft bright highlight that sits where the scene's sun
/// reflects toward the eye. As the camera orbits (or the slab moves), the glint slides across the surface, so
/// light genuinely *plays* over the slab instead of it reading as a flat quad. `sun` is the world sun dir;
/// `hw`/`hu` are the slab half-width/half-height. Intensity rises when the sun faces the slab toward the eye.
fn slab_glint(graph: &mut Vec<crate::mesh3d::Mesh3>, c: crate::spatial::Vec3, right: crate::spatial::Vec3,
              up: crate::spatial::Vec3, toward: crate::spatial::Vec3, hw: f32, hu: f32,
              sun: crate::spatial::Vec3, tglow: f32) {
    let dot = |a: crate::spatial::Vec3, b: crate::spatial::Vec3| a.x * b.x + a.y * b.y + a.z * b.z;
    let du = dot(right, sun).clamp(-1.0, 1.0);
    let dv = dot(up, sun).clamp(-1.0, 1.0);
    // brightness varies with how the slab faces the sun (either sign — the sign convention is refined with the
    // viewer), but stays present so it reads as a moving sheen rather than blinking on/off.
    let inten = 0.9 + 0.7 * dot(toward, sun).abs();
    let (gu, gv) = (du * hw * 0.72, dv * hu * 0.72);
    let gc = crate::spatial::Vec3::new(c.x + right.x * gu + up.x * gv - toward.x * 0.03,
                                       c.y + right.y * gu + up.y * gv - toward.y * 0.03,
                                       c.z + right.z * gu + up.z * gv - toward.z * 0.03);
    let r = (hw.min(hu) * 0.5).clamp(0.05, 0.4);
    let mut glint = crate::mesh3d::uv_sphere(gc, r, 6, 4, egui::Color32::from_rgb(255, 251, 236));
    glint.emissive = inten * tglow.max(0.6);
    crate::mesh3d::set_alpha(&mut glint, 135);
    graph.push(glint);
}

/// A subtle per-scene emissive undulation (~0.85..1.25) so physical UI slabs catch the scene's light rhythm —
/// light rippling on Atlantis glass, a slow sway in Forest, an icy glint in Frozen — instead of reading flat.
fn slab_sheen(anim: crate::mesh3d::TextAnim, t: f32) -> f32 {
    use crate::mesh3d::TextAnim::*;
    match anim {
        Ripple => 1.0 + 0.14 * (t * 1.6).sin(),
        Sway => 1.0 + 0.08 * (t * 1.1).sin(),
        Shimmer => 1.0 + 0.22 * ((t * 3.0).sin() * 0.5 + 0.5).powi(4),
        Drift => 1.0 + 0.10 * (t * 0.8).sin(),
        Fade => 0.9 + 0.22 * (0.5 + 0.5 * (t * 1.4).sin()),
        Pulse => 1.0 + 0.16 * (t * 2.2).sin(),
        Plain => 1.0,
    }
}

/// Local screen scale (pixels per world unit) at `cen`'s depth, along right and up — for sizing physical UI
/// slabs to a target fraction of the canvas so they don't blow up or shrink with window size.
fn px_per_world(cam: &crate::spatial::OrbitCam, cen: crate::spatial::Vec3, right: crate::spatial::Vec3, up: crate::spatial::Vec3, cx: f32, cy: f32, scale: f32) -> (f32, f32) {
    let pt = |p: crate::spatial::Vec3| cam.project(p, cx, cy, scale).map(|(x, y, _)| (x, y));
    let add = |a: crate::spatial::Vec3, b: crate::spatial::Vec3| crate::spatial::Vec3::new(a.x + b.x, a.y + b.y, a.z + b.z);
    match (pt(cen), pt(add(cen, right)), pt(add(cen, up))) {
        (Some(p0), Some(pr), Some(pu)) => (((pr.0 - p0.0).powi(2) + (pr.1 - p0.1).powi(2)).sqrt().max(1.0),
                                           ((pu.0 - p0.0).powi(2) + (pu.1 - p0.1).powi(2)).sqrt().max(1.0)),
        _ => (scale.max(1.0), scale.max(1.0)),
    }
}

/// Unproject a screen point `(ax, ay)` back to a world position on the plane through `cen` spanned by
/// `right`/`up` (where the billboarded physical UI lives), so UI components can be ANCHORED to screen
/// positions that adapt to canvas size instead of using fixed world offsets. Falls back to `cen`.
fn screen_to_world(cam: &crate::spatial::OrbitCam, cen: crate::spatial::Vec3, right: crate::spatial::Vec3, up: crate::spatial::Vec3, cx: f32, cy: f32, scale: f32, ax: f32, ay: f32) -> crate::spatial::Vec3 {
    let add = |a: crate::spatial::Vec3, b: crate::spatial::Vec3| crate::spatial::Vec3::new(a.x + b.x, a.y + b.y, a.z + b.z);
    let (p0, pr, pu) = match (cam.project(cen, cx, cy, scale), cam.project(add(cen, right), cx, cy, scale), cam.project(add(cen, up), cx, cy, scale)) {
        (Some(a), Some(b), Some(c)) => (a, b, c),
        _ => return cen,
    };
    let (drx, dry) = (pr.0 - p0.0, pr.1 - p0.1);
    let (dux, duy) = (pu.0 - p0.0, pu.1 - p0.1);
    let det = drx * duy - dux * dry;
    if det.abs() < 1e-4 { return cen; }
    let (tx, ty) = (ax - p0.0, ay - p0.1);
    let wr = (tx * duy - dux * ty) / det;
    let wu = (drx * ty - tx * dry) / det;
    crate::spatial::Vec3::new(cen.x + right.x * wr + up.x * wu, cen.y + right.y * wr + up.y * wu, cen.z + right.z * wr + up.z * wu)
}

/// Map a scene/theme name to its text-animation idiom, so each scene animates its UI text in character.
fn text_anim_for(name: &str) -> crate::mesh3d::TextAnim {
    use crate::mesh3d::TextAnim::*;
    match name {
        "Atlantis" => Ripple,
        "Forest" | "Treehouse" | "Canopy" => Sway,
        "Frozen Bifrost" => Shimmer,
        "Cloudscape" => Drift,
        "Mistfall" => Fade,
        "Folding@home" => Pulse,
        _ => Plain, // Archive / Slate / Alley and any others: the steady twinkle
    }
}

/// Word-wrap `text` into at most `max_lines` lines of ≤`cols` chars, for physical `label_cells` panels
/// (which render one line at a time). Wraps on whitespace; hard-splits words longer than `cols`; collapses
/// internal newlines/spaces; appends `…` to the last line when the text was truncated. Pure + testable.
/// How many wrapped lines the physical-UI reading plaque shows at once — the rest is reached by
/// scrolling (jk / impakt-cam glide) rather than being truncated with a trailing "…".
const PLAQUE_WINDOW: usize = 4;

fn wrap_label(text: &str, cols: usize, max_lines: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut truncated = false;
    'outer: for word in text.split_whitespace() {
        let mut word = word;
        // hard-split words longer than a line
        while word.chars().count() > cols {
            if !cur.is_empty() { lines.push(std::mem::take(&mut cur)); if lines.len() >= max_lines { truncated = true; break 'outer; } }
            let head: String = word.chars().take(cols).collect();
            lines.push(head);
            if lines.len() >= max_lines { truncated = true; break 'outer; }
            let skip = cols; word = &word[word.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(word.len())..];
        }
        if cur.is_empty() { cur = word.to_string(); }
        else if cur.chars().count() + 1 + word.chars().count() <= cols { cur.push(' '); cur.push_str(word); }
        else { lines.push(std::mem::take(&mut cur)); if lines.len() >= max_lines { truncated = true; break; } cur = word.to_string(); }
    }
    if !truncated && !cur.is_empty() && lines.len() < max_lines { lines.push(cur); }
    else if !cur.is_empty() { truncated = true; }
    if truncated {
        if let Some(last) = lines.last_mut() {
            while last.chars().count() >= cols { last.pop(); }
            last.push('…');
        }
    }
    lines
}

/// A translucent "frosted glass" frame for chrome (top control bar, bottom read-out) floating OVER the
/// full-bleed scene — a dark semi-transparent fill + a faint light rim so it reads as a panel of smoked
/// glass on top of the 3D view. (egui can't sample a true backdrop blur; this is the "for now" glass.)
fn glass_frame() -> egui::Frame {
    egui::Frame {
        fill: egui::Color32::from_rgba_unmultiplied(10, 13, 20, 190),
        corner_radius: theme::r(10),
        inner_margin: egui::Margin::symmetric(10, 6),
        stroke: egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 24)),
        ..Default::default()
    }
}
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
    let strong = ui.visuals().strong_text_color();
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
        if r.link { fmt.underline = egui::Stroke::new(1.0, accent); }
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
                egui::Stroke::new(2.5, bar),
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
];
/// Look up a persona's full system prompt by key (the status-bar persona popup switches with this).
fn persona_by_key(key: &str) -> Option<&'static str> { PERSONAS.iter().find(|p| p.0 == key).map(|p| p.2) }

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
    hide_bottom_bar: bool,         // hide the whole composer/files/suggestions strip — reclaims the
                                    // full window height for the physical UI (spatial_view), which was
                                    // otherwise cramped/cut off behind it with no way to get it back.
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
    gitmind_session: Option<crate::gitmind_session::Session>, // a loaded .gitmind /tmp work session, if any
    agent_files: Vec<crate::workspace_exec::FileEvent>, // live feed of files the agent/drops touched in the workspace
    agent_plan: Vec<(String, bool)>, // the agent's committed plan (2): (step text, done) — rendered as an in-scene checklist
    agent_run: Option<crate::workspace_exec::AgentRun>, // active NON-BLOCKING agent driver (channel-driven state machine)
    agent_fan: Option<crate::workspace_exec::AgentFan>, // active MULTI-branch fan (parallel agent branches, round-robin)
    agent_candidates: Vec<crate::workspace_exec::BranchSummary>, // folded branch summaries to prune to the keeper
    fan_is_docs: bool, // the active/just-finished fan is a DOC fan → suggest :merge-docs (union) over :pick (keep one)
    agent_pending_submit: Option<String>, // next agent prompt to hand the inference worker (drained in sync_worker)
    agent_task: String, // the in-flight :agent task text (for the snapshot message + reaction seeding)
    lanes: Option<crate::lanes::LanePool>, // :lanes N — concurrent generation lanes over ONE resident model; fan branches decode in parallel through this instead of round-robin on the single worker
    lane_jobs: std::collections::HashMap<crate::lanes::JobId, usize>, // in-flight pool job → fan branch (routes tagged Token/Done events)
    lanes_ready: bool, // the pool's async model load completed (LaneEvt::Ready seen)
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
    preview: Option<usize>,                 // index of the generated file whose body is expanded
    auto_continue: bool,                    // decode a fresh reply continuously up to the reply ceiling
    stepwise: bool,                         // checkpoint mode: model pauses at ⏸ STEP k/n markers, harness auto-continues
    step_last_paused: u32,                  // dedup: highest step number already paused on this reply
    step_continues: u32,                    // auto-continues issued for this reply (hard-capped)
    step_pending_continue: bool,            // a step pause finished → issue the continue when the stream closes
    step_arm_continue: bool,                // set by the Done handler; consumed by the frame loop
    step_shell: Option<std::sync::mpsc::Receiver<String>>, // in-flight cargo test at a step pause
    step_inject: Option<String>,            // shell results to splice into the next continue
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
    phys: crate::physui::Panel,             // in-scene physical control dock (Overview/Tour/Freecam as 3D buttons)
    phys_anim: Vec<crate::physui::ControlAnim>, // per-dock-control eased press/hover/lit feel (indexed by DOCK_IDS)
    phys_t: Option<std::time::Instant>,     // last dock-anim tick (for dt); None until the first frame
    slider_drag: Option<(usize, f32)>,      // while dragging a physical settings slider: (dial id, last pointer x)
}
impl Default for Chat {
    fn default() -> Self {
        Self {
            msgs: Vec::new(), input: String::new(),
            context: LONG_HORIZON.into(),
            threading: true, show_context: false, attach_log: true, fix_mode: false,
            right_page: RightPage::Prompting, coord_mode: crate::coordinator::Mode::Barebones,
            verify_job: None,
            verify_net: false,
            show_files: false,
            hide_bottom_bar: false,
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
            gitmind_session: None,
            agent_files: Vec::new(),
            agent_plan: Vec::new(),
            agent_run: None,
            agent_fan: None,
            agent_candidates: Vec::new(),
            fan_is_docs: false,
            agent_pending_submit: None,
            agent_task: String::new(),
            lanes: None, lane_jobs: std::collections::HashMap::new(), lanes_ready: false,
            local_model_path: None,
            rep_monitor: crate::repetition::RepeatMonitor::default(),
            stream: None, last: None, ctx: Ctx::default(),
            backend: BackendKind::preferred(), sent_params: None, sent_backend: None, pending_cancel: false,
            ws_note: None, save_picker: Default::default(),
            attach_picker: Default::default(), save_dir: None, last_saved: None, preview: None,
            auto_continue: true, stepwise: false, step_last_paused: 0, step_continues: 0, step_pending_continue: false, step_arm_continue: false, step_shell: None, step_inject: None, hit_cap: false,
            checkpoints: Vec::new(), show_checkpoints: false, checked_restore: false, recover: None,
            vault: autosave_path().and_then(|p| p.parent().map(|d| d.join("vault")))
                .and_then(|d| crate::vault::Vault::open(d).ok()),
            head: None, show_history: false, file_hist: None,
            template: TemplateKind::Llama3,
            phys: phys_dock(),
            phys_anim: vec![crate::physui::ControlAnim::default(); DOCK_IDS.len()],
            phys_t: None,
            slider_drag: None,
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
                Evt::Loaded(Ok(info)) => { self.template = info.template; return; }
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
                                self.note = format!("agent: turn {} · {} file(s)…", r.turns(), r.events().len());
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
                Evt::Loaded(Ok(info)) => { self.template = info.template; return; }
                Evt::Loaded(Err(_)) => { return; }
            }
        }
        match e {
            Evt::Token(t) => {
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
                                let cont = crate::stepwise::should_continue(&mark, m.text(), self.step_continues, 12);
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
                let idx = self.stream.as_ref().map(|s| s.idx);
                if let Some(i) = idx {
                    if let Some(m) = self.msgs.get_mut(i) {
                        if m.text().is_empty() { *m.cur_mut() = "[stopped]".into(); }
                    }
                }
                self.last = Some((tokens, secs));
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
        let mut sys = neutralize_markers(&self.full_system(doc));
        if self.stepwise { sys.push_str(crate::stepwise::STEPWISE_PROMPT); }
        let sys = sys;
        match self.backend {
            BackendKind::Wgpu => match self.template {
                TemplateKind::ChatML => self.build_prompt_chatml(&sys, upto),
                TemplateKind::Llama3 => self.build_prompt_llama3(&sys, upto),
                TemplateKind::Gemma => self.build_prompt_gemma(&sys, upto),
                TemplateKind::Plain => self.build_prompt_plain(&sys, upto),
            },
            // llama.cpp path is otherwise Plain, but Gemma needs its own turn markers to stop correctly.
            _ if self.template == TemplateKind::Gemma => self.build_prompt_gemma(&sys, upto),
            _ => self.build_prompt_plain(&sys, upto),
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
        let sys = sys;
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

    fn send(&mut self, infer: &Inference, doc: &crate::analysis::Doc, ui: &egui::Ui) {
        // Composer command routing: a line the user submits that begins with ':' is executed as a
        // command HERE, not sent to the model. '::text' escapes a literal message that starts with a
        // colon. Everything else is a normal chat turn.
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
    fn start_generation(&mut self, qt: String, parent: Option<usize>, infer: &Inference, doc: &crate::analysis::Doc) {
        // fresh reply → fresh stepwise accounting
        self.step_last_paused = 0; self.step_continues = 0; self.step_pending_continue = false; self.step_arm_continue = false;
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
        // Template: trust the worker's LIVE detected template over per-tab cached state. A tab created
        // after the model loaded (or whose Evt::Loaded another drain consumed) otherwise keeps a stale
        // default and silently builds Llama-3 prompts for a ChatML model — the markers byte-split
        // (prompt diag: "0 special tokens"), the turn structure never reaches the model, and output
        // degrades to word salad (seen on every Qwen3 while Llama models worked).
        if let Some(t) = infer.detected_template() {
            if self.template != t { self.template = t; }
        }
        let key = (self.ctx.n_ctx, self.ctx.max_tokens, (self.ctx.temperature * 100.0) as i32, self.ctx.history_turns);
        if self.sent_params != Some(key) { infer.set_params(self.params()); self.sent_params = Some(key); }
        // Drain a queued agent prompt (initial or next-turn) to the worker — this is where the non-blocking
        // agent actually submits generations, since `infer` is in scope here every frame.
        if let Some(prompt) = self.agent_pending_submit.take() { infer.generate(prompt); }
    }

    /// Finalize a non-blocking agent run: snapshot the workspace, surface the outcome, clear the driver.
    fn finish_agent_run(&mut self, stop: crate::workspace_exec::Stop) {
        let (files, plan, verified, turns) = match self.agent_run.take() {
            Some(r) => (r.events().to_vec(), r.plan().steps.iter().map(|s| (s.text.clone(), s.done)).collect::<Vec<_>>(), r.verified(), r.turns()),
            None => return,
        };
        self.agent_files = files;
        self.agent_plan = plan;
        let touched = self.agent_files.len();
        let done = self.agent_plan.iter().filter(|(_, d)| *d).count();
        let total = self.agent_plan.len();
        let stopdesc = match &stop {
            crate::workspace_exec::Stop::Done => "done".to_string(),
            crate::workspace_exec::Stop::StepLimit => format!("hit step limit ({turns} turns)"),
            crate::workspace_exec::Stop::Stalled => format!("stopped — model was looping ({turns} turns)"),
            crate::workspace_exec::Stop::ModelError(e) => format!("model error: {e}"),
        };
        let snap = self.gitmind_session.as_ref()
            .and_then(|s| crate::mount::snapshot(&s.ws, &format!("agent: {}", self.agent_task)).ok())
            .unwrap_or(false);
        let vnote = if verified { " · ✓ verified" } else { " · ⚠ unverified" };
        let snap_note = if snap { " · snapshotted" } else { "" };
        self.note = format!("agent: {stopdesc} · plan {done}/{total} · {touched} file(s){vnote}{snap_note} · :reapply to land");
    }

    /// Launch a fan of agent branches with `cfg`, seeding goals from `task`. Shared by :fan-agent and :docs.
    fn start_fan(&mut self, task: &str, cfg: crate::workspace_exec::AgentConfig, doc_fan: bool, label: &str) {
        let task = task.trim().to_string();
        if task.is_empty() {
            self.note = format!(":{label} needs a task");
        } else if self.gitmind_session.is_none() {
            self.note = format!(":{label} — load a workspace first (:load-gitmind <path>)");
        } else if self.agent_run.is_some() || self.agent_fan.is_some() || self.stream.is_some() {
            self.note = format!(":{label} — the model is busy (:stop to cancel)");
        } else {
            let base_goal = self.msgs.iter().rev().find(|m| m.role == Role::User).map(|m| m.text().to_string()).unwrap_or_else(|| task.clone());
            let goals = crate::workspace_exec::seed_agent_branches(&task, &base_goal, 4);
            let goals = if goals.is_empty() { vec![task.clone()] } else { goals };
            let session = self.gitmind_session.as_ref().unwrap();
            let mount = match crate::mount::detect(&session.project) { Ok(m) => m, Err(e) => { self.note = format!(":{label} — {e}"); return; } };
            let fan_root = std::env::temp_dir().join("logscope-fan");
            let mut specs = Vec::new();
            for (i, goal) in goals.iter().enumerate() {
                match crate::mount::isolate(&mount, &fan_root.join(format!("branch{i}"))) {
                    Ok(ws) => specs.push((ws.dir.clone(), goal.clone(), format!("branch {}", i + 1))),
                    Err(e) => { self.note = format!(":{label} — isolate failed: {e}"); return; }
                }
            }
            let diff_fn = Box::new(|dir: &std::path::Path| -> String {
                std::process::Command::new("git").arg("-C").arg(dir).args(["diff", "logscope-base"])
                    .output().ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).chars().take(8000).collect())
                    .unwrap_or_default()
            });
            let mut fan = crate::workspace_exec::AgentFan::new(specs, &cfg, diff_fn);
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
                self.agent_fan = Some(fan);
                let ready = if self.lanes_ready { "decoding in parallel" } else { "queued (lanes still loading)" };
                self.note = format!("{label}: {n} branches — {launched} {ready} on {lane_n} lane(s)");
            } else {
                match fan.next() {
                    crate::workspace_exec::FanStep::Submit { prompt, .. } => {
                        self.agent_pending_submit = Some(prompt);
                        let n = fan.len();
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
                crate::lanes::LaneEvt::Ready { lanes, model, .. } => {
                    self.lanes_ready = true;
                    self.note = format!("lanes: {lanes} × {model} up — one weights copy, {lanes} KV caches; fans now decode {lanes} branches at once");
                    // Widen a fan that's already pool-driven (jobs queued before the load finished).
                    if !self.lane_jobs.is_empty() {
                        if let Some(f) = self.agent_fan.as_mut() { f.set_width(lanes); steps.extend(f.fill()); }
                    }
                }
                crate::lanes::LaneEvt::LoadError(e) => { self.note = format!("lanes: load failed — {e}"); dead = true; }
                crate::lanes::LaneEvt::Token { job, text } => {
                    if let Some(&b) = self.lane_jobs.get(&job) {
                        if let Some(f) = self.agent_fan.as_mut() { f.on_token_branch(b, &text); }
                    }
                }
                crate::lanes::LaneEvt::Done { job, .. } => {
                    if let Some(b) = self.lane_jobs.remove(&job) {
                        if let Some(f) = self.agent_fan.as_mut() { steps.extend(f.on_done_branch(b)); }
                    }
                }
                crate::lanes::LaneEvt::Error { job, err } => {
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
    fn spatial_ui(&mut self, ui: &mut egui::Ui, mut model: Option<&mut ModelState>) -> bool {
        let mut send_now = false;
        // DRAG-AND-DROP INGEST: files dropped onto the scene land in the active workspace session. This is
        // what makes the tmp workspace a real drop target — you can weave external files into the agent's
        // codebase by dropping them. Desktop drops carry a path; some stacks carry bytes; we handle both.
        if self.gitmind_session.is_some() {
            let drops = ui.ctx().input(|i| i.raw.dropped_files.clone());
            if !drops.is_empty() {
                let work_dir = self.gitmind_session.as_ref().unwrap().work_dir().to_path_buf();
                let mut ws = crate::workspace_exec::Workspace::new(work_dir);
                let mut ok = 0usize;
                for f in &drops {
                    let name = f.name.rsplit(['/', '\\']).next().filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .or_else(|| f.path.as_ref().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())))
                        .unwrap_or_else(|| "dropped.bin".into());
                    let dest = format!("dropped/{name}");
                    let res = if let Some(bytes) = &f.bytes {
                        ws.ingest(&dest, bytes)
                    } else if let Some(path) = &f.path {
                        match std::fs::read(path) { Ok(b) => ws.ingest(&dest, &b), Err(e) => crate::workspace_exec::ToolResult { ok: false, output: e.to_string() } }
                    } else {
                        crate::workspace_exec::ToolResult { ok: false, output: "empty drop".into() }
                    };
                    if res.ok { ok += 1; }
                }
                for ev in ws.take_events() { self.agent_files.push(ev); }
                self.note = format!("dropped {ok}/{} file(s) into the workspace → dropped/", drops.len());
                ui.ctx().request_repaint();
            }
        }

        let g = self.convo_graph(self.msgs.len());
        if g.nodes.is_empty() {
            if !self.backdrop {
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                ui.label(RichText::new("Spatial view").color(theme::DIM()).strong());
                ui.label(RichText::new("Nodes appear here as you converse — drag to orbit, scroll to zoom.").color(theme::FAINT()).small());
            });
            }
            return false;
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
            // Per-node TOUR DWELL weight from message length: the camera lingers on longer messages (time to
            // read) and moves quicker past short ones. BOUNDED by a z-score clamp to [0.6, 2.4] so the pacing
            // stays even (the "bounded mean/stddev-wise to keep the flow" request) rather than crawling on one
            // huge turn or racing past a cluster of short ones. Indexed to match `pos` (g.nodes order).
            let lens: Vec<f32> = g.nodes.iter().map(|nd| self.msgs.get(nd.id).map(|m| m.text().chars().count() as f32).unwrap_or(0.0)).collect();
            let mean = if n > 0 { lens.iter().sum::<f32>() / n as f32 } else { 0.0 };
            let sd = if n > 0 { (lens.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / n as f32).sqrt().max(1.0) } else { 1.0 };
            self.tour_dwell = lens.iter().map(|&l| (1.0 + 0.6 * ((l - mean) / sd)).clamp(0.6, 2.4)).collect();
        }
        let pos = {
            let mut p = self.fold_pos.clone();
            // SEAT THE CONVERSATION ON THE FLOOR: the flow meander + sibling fanning can dip a node below a scene's
            // floor (≈ −3.2), so the strand pokes "into the ground". Lift the whole graph so its LOWEST node sits a
            // little above the floor — and only ever lift UP (never push a high graph down), so open scenes are
            // unaffected. This raises the walk to about eye level for the fly camera that rides the strand.
            let min_y = p.iter().map(|q| q.y).fold(f32::INFINITY, f32::min);
            if min_y.is_finite() {
                // Mistfall floats the conversation HIGH over its deep lake (the void between the low water and the
                // strand is what gives the cave its depth); other scenes seat just above their floor (~−3.2).
                let target_bottom = if theme::active().name == "Mistfall" { 1.6 } else { -1.6 };
                let lift = (target_bottom - min_y).max(0.0);
                for q in &mut p { q.y += lift; }
            }
            p
        };

        // HALL ROUTING: in hall-based scenes (Atlantis) the strand should run DOWN the open hallway lane along x,
        // not balloon into the furnished rotunda. Scale the graph laterally so its WIDEST node fits inside the clear
        // central corridor (between the colonnade rows + book-stacks), so it threads the open lane instead of the
        // columns. Scene-aware: round/open scenes keep their full spread. The geom harness then measures the drop.
        // Atlantis no longer squishes the strand laterally: its rotunda is now an OVAL that EXPANDS to contain the
        // graph's full branch span (see scene3d::atlantis + hall_dims), so the conversation spreads naturally rather
        // than being crammed onto the central axis. Other scenes already keep their full spread.

        // OBSTACLE FLEX: weave the strand AROUND solid scenery instead of clipping through it. The geom harness
        // showed tuning can't fix this (the strand balls up among thousands of obstacles), so push each node out
        // of any mid-size solid object it overlaps — along the smaller x/z penetration, biased to z so the time
        // axis (x) stays ordered, clamped near its spine spot. Cached per (node-count, env): the scene is
        // deterministic for a conversation, so this runs once, not per frame.
        let pos = {
            let key = (n, crate::scene3d::env_params());
            if self.flex_key.as_ref() != Some(&key) {
                let orig = pos.clone();
                let mut fp = pos.clone();
                let mut obs: Vec<([f32; 2], [f32; 2], [f32; 2])> = Vec::new(); // (x[lo,hi], z[lo,hi], y[lo,hi])
                crate::scene3d::with_env(theme::active().name, 0.0, |meshes| {
                    for m in meshes {
                        if m.verts.is_empty() { continue; }
                        let (mut blo, mut bhi) = (m.verts[0], m.verts[0]);
                        for v in &m.verts { blo.x = blo.x.min(v.x); blo.y = blo.y.min(v.y); blo.z = blo.z.min(v.z); bhi.x = bhi.x.max(v.x); bhi.y = bhi.y.max(v.y); bhi.z = bhi.z.max(v.z); }
                        let hx = (bhi.x - blo.x).max(bhi.z - blo.z); let vy = bhi.y - blo.y;
                        let fp_min = (bhi.x - blo.x).min(bhi.z - blo.z);
                        let hard = matches!(m.mat as u32, 1 | 2); // wood/stone only — leaves/water/paper are passable
                        if hard && hx < 14.0 && vy > 1.5 && fp_min > 0.4 { obs.push(([blo.x, bhi.x], [blo.z, bhi.z], [blo.y, bhi.y])); } // local solids, not terrain slabs
                    }
                });
                let r = 0.5f32; let cap = 8.0f32;
                for _ in 0..12 {
                    for (qi, q) in fp.iter_mut().enumerate() {
                        for (ox, oz, oy) in &obs {
                            if q.x + r > ox[0] && q.x - r < ox[1] && q.z + r > oz[0] && q.z - r < oz[1] && q.y > oy[0] - r && q.y < oy[1] + r {
                                let (px_lo, px_hi) = ((q.x + r) - ox[0], ox[1] - (q.x - r));
                                let (pz_lo, pz_hi) = ((q.z + r) - oz[0], oz[1] - (q.z - r));
                                // smallest penetration wins, but penalise x-moves (×1.6) so the strand goes AROUND
                                // sideways and keeps its time order along x
                                let cands = [(px_lo * 1.6, 0u8), (px_hi * 1.6, 1), (pz_lo, 2), (pz_hi, 3)];
                                let mut best = (f32::INFINITY, 4u8);
                                for &(v, a) in &cands { if v < best.0 { best = (v, a); } }
                                match best.1 { 0 => q.x -= px_lo, 1 => q.x += px_hi, 2 => q.z -= pz_lo, _ => q.z += pz_hi }
                            }
                        }
                        // keep each node near its spine position so the strand doesn't scatter
                        let (dx, dz) = (q.x - orig[qi].x, q.z - orig[qi].z);
                        let d = (dx * dx + dz * dz).sqrt();
                        if d > cap { q.x = orig[qi].x + dx / d * cap; q.z = orig[qi].z + dz / d * cap; }
                    }
                }
                self.flex_pos = fp; self.flex_key = Some(key);
            }
            if self.flex_pos.len() == n { self.flex_pos.clone() } else { pos }
        };

        // CANOPY HANG: in the Canopy scene the conversation DANGLES from the trees rather than resting among them.
        // A cone-scan upward from every third node finds a branch overhead and "ties a rope" — that node is pinned
        // up at branch height; the nodes between anchors then sag under gravity (catenary relaxation), so the strand
        // hangs in draped swags from the canopy instead of being squished flat by the surroundings. Distinct shape.
        let pos = if theme::active().name == "Canopy" && n > 2 {
            use crate::spatial::Vec3 as V3;
            let mut p = pos.clone();
            let floor = -3.2f32;
            // per-anchor branch height from a cone-scan: sample the canopy solids above the node and tie on just
            // beneath the lowest branch found (fall back to a default canopy height when the cone finds nothing).
            let mut obs: Vec<(V3, V3)> = Vec::new(); // (lo, hi) AABBs of canopy solids
            crate::scene3d::with_env("Canopy", 0.0, |meshes| {
                for m in meshes {
                    if m.verts.is_empty() || !matches!(m.mat as u32, 1 | 2 | 4) { continue; } // wood/stone/moss = branches & platforms
                    let (mut lo, mut hi) = (m.verts[0], m.verts[0]);
                    for v in &m.verts { lo.x = lo.x.min(v.x); lo.y = lo.y.min(v.y); lo.z = lo.z.min(v.z); hi.x = hi.x.max(v.x); hi.y = hi.y.max(v.y); hi.z = hi.z.max(v.z); }
                    if hi.y > floor + 3.0 { obs.push((lo, hi)); } // only things up in the canopy
                }
            });
            let cone = |x: f32, z: f32| -> f32 {                       // lowest branch overhead within a widening cone
                let mut best = f32::INFINITY;
                for (lo, hi) in &obs {
                    let r = 1.5 + (hi.y - floor) * 0.25;               // cone widens with height
                    if x > lo.x - r && x < hi.x + r && z > lo.z - r && z < hi.z + r && lo.y > floor + 2.0 { best = best.min(lo.y); }
                }
                if best.is_finite() { (best - 0.8).min(floor + 11.0) } else { floor + 7.5 } // tie on just under the branch
            };
            for (i, q) in p.iter_mut().enumerate() {
                if i % 3 == 0 { q.y = cone(q.x, q.z) + (((i as f32 * 12.9898).sin() * 43758.5453).fract().abs() - 0.5) * 1.2; } // anchor: roped to the branch
            }
            for _ in 0..60 {                                          // GRAVITY relaxation: chains sag between anchors
                for i in 1..p.len() - 1 {
                    if i % 3 == 0 { continue; }
                    let chord = (p[i - 1].y + p[i + 1].y) * 0.5;
                    p[i].y += (chord - 0.7 - p[i].y) * 0.5;           // pull below the chord → droop
                }
                for q in p.iter_mut() { if q.y < floor + 0.6 { q.y = floor + 0.6; } } // never sag through the floor
            }
            p
        } else { pos };

        // GEOMETRY DUMP (LOGSCOPE_GEOM_DUMP=1) for the geometric self-check harness (tools/geom_check.py): writes
        // scene object AABBs + graph node positions + the scene AABB to /tmp/scene_geom_<theme>.json, so bbox-
        // overlap + range tests can measure graph↔scene fit (into-the-ground, object clipping, out-of-bounds,
        // spine/axis alignment) objectively instead of eyeballing renders.
        if std::env::var("LOGSCOPE_GEOM_DUMP").is_ok() {
            use std::fmt::Write as _;
            let mut objs = String::new();
            let (mut smin, mut smax) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            crate::scene3d::with_env(theme::active().name, 0.0, |meshes| {
                for (i, m) in meshes.iter().enumerate() {
                    if m.verts.is_empty() { continue; }
                    let (mut lo, mut hi) = (m.verts[0], m.verts[0]);
                    for vtx in &m.verts {
                        lo.x = lo.x.min(vtx.x); lo.y = lo.y.min(vtx.y); lo.z = lo.z.min(vtx.z);
                        hi.x = hi.x.max(vtx.x); hi.y = hi.y.max(vtx.y); hi.z = hi.z.max(vtx.z);
                    }
                    smin[0] = smin[0].min(lo.x); smin[1] = smin[1].min(lo.y); smin[2] = smin[2].min(lo.z);
                    smax[0] = smax[0].max(hi.x); smax[1] = smax[1].max(hi.y); smax[2] = smax[2].max(hi.z);
                    let _ = write!(objs, "{}{{\"i\":{},\"lo\":[{:.3},{:.3},{:.3}],\"hi\":[{:.3},{:.3},{:.3}],\"mat\":{},\"emis\":{:.2}}}",
                        if objs.is_empty() { "" } else { "," }, i, lo.x, lo.y, lo.z, hi.x, hi.y, hi.z, m.mat as u32, m.emissive);
                }
            });
            let mut nodes = String::new();
            for (i, q) in pos.iter().enumerate() {
                let _ = write!(nodes, "{}{{\"i\":{},\"p\":[{:.3},{:.3},{:.3}]}}", if nodes.is_empty() { "" } else { "," }, i, q.x, q.y, q.z);
            }
            let json = format!(
                "{{\"theme\":\"{}\",\"nodes\":{},\"bounds\":{{\"lo\":[{:.2},{:.2},{:.2}],\"hi\":[{:.2},{:.2},{:.2}]}},\"objects\":[{}],\"graph\":[{}]}}",
                theme::active().name, pos.len(), smin[0], smin[1], smin[2], smax[0], smax[1], smax[2], objs, nodes);
            let path = format!("/tmp/scene_geom_{}.json", theme::active().name.replace(' ', "_"));
            let _ = std::fs::write(path, json);
        }

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
            // procedural TREE character (Forest), seed-derived so each conversation grows a distinct wood, with a
            // light branchiness influence on the fork factor
            let tfrac = |salt: u32| -> f32 { let mut h = seed ^ salt.wrapping_mul(0x9E37_79B1); h ^= h >> 13; h = h.wrapping_mul(0xC2B2_AE35); h ^= h >> 16; (h & 0xFFFF) as f32 / 65535.0 };
            let tree_size = 0.93 + tfrac(0x71) * 0.14;        // 0.93..1.07 mild overall scale (compounds with h/g)
            let tree_height = 0.92 + tfrac(0x72) * 0.18;      // 0.92..1.10 trunk length factor (tall trees reach the upper strand → clip)
            let tree_girth = 0.85 + tfrac(0x73) * 0.40;       // 0.85..1.25 trunk radius factor (girth varies more freely)
            let tree_branch = (3 + branches / 8).clamp(3, 5); // only genuinely forky chats bushier (branches are obstacles)
            let branch_spread = 0.5 + tfrac(0x75) * 0.35;     // 0.50..0.85 second branching factor: how wide forks splay
            let prop_density = (0.75 + tfrac(0x76) * 0.5 + (n as f32 / 240.0).min(0.35)).clamp(0.7, 1.6); // street props scale with seed + size
            crate::scene3d::set_env_params(crate::scene3d::EnvParams {
                nodes: n as u32, branches, segments: segs.len() as u32, seed,
                ext_x: ext_x.ceil() as u32, ext_z: ext_z.ceil() as u32, bear,
                color_salience, tree_size, tree_height, tree_girth, tree_branch, branch_spread, prop_density,
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

        // (The 2D control bar was removed — Overview/Tour/Freecam/Prune/Unprune are physical dock buttons now.)

        let total = ui.available_size();
        // Full-bleed: the scene fills the whole window; the top control bar and the bottom read-out float over
        // it as translucent glass overlays (Areas), so nothing reserves vertical space any more.
        let canvas_h = total.y.max(180.0);
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
            // EASE toward the centroid + fit distance rather than SNAPPING every frame. Snapping made the camera
            // twitch/ricochet as the strand breathes (centroid + bounding radius shift each frame); a gentle lerp
            // tracks it as a calm drift that combines the graph's motion with the framing of its bounds. Converges
            // in a few frames, so headless captures (which honour an explicit dist via spatial_zoomed) are unaffected.
            let k = 0.09;
            self.orbit.target.x += (c.x - self.orbit.target.x) * k;
            self.orbit.target.y += (c.y - self.orbit.target.y) * k;
            self.orbit.target.z += (c.z - self.orbit.target.z) * k;
            self.orbit.roll += (0.0 - self.orbit.roll) * k; // level the horizon when not in a cinematic shot
            if !self.spatial_zoomed {
                let fit = crate::spatial::fit_distance(crate::spatial::bounding_radius(&pos, c));
                self.orbit.dist += (fit - self.orbit.dist) * k;
            }
            ui.ctx().request_repaint(); // keep easing until settled
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

        // FREECAM: free-flight through the scene. WASD move along the look basis, Q/E down/up, drag looks (above),
        // Shift boosts. Moving the target translates the eye with it (dist held), so you fly wherever you look.
        // Gated on hovering the canvas so WASD never leaks into the composer; repaints continuously to poll keys.
        if self.freecam {
            use crate::spatial::Vec3 as V3;
            if resp.hovered() || resp.dragged() {
                let (mv, dt, boost) = ui.input(|i| {
                    let (cy, sy) = (self.orbit.yaw.cos(), self.orbit.yaw.sin());
                    let (cp, sp) = (self.orbit.pitch.cos(), self.orbit.pitch.sin());
                    let fwd = V3::new(cp * sy, -sp, -cp * cy);   // eye→target look direction (fly where you look)
                    let right = V3::new(cy, 0.0, sy);            // horizontal strafe
                    let mut mv = V3::new(0.0, 0.0, 0.0);
                    if i.key_down(egui::Key::W) { mv.x += fwd.x; mv.y += fwd.y; mv.z += fwd.z; }
                    if i.key_down(egui::Key::S) { mv.x -= fwd.x; mv.y -= fwd.y; mv.z -= fwd.z; }
                    if i.key_down(egui::Key::D) { mv.x += right.x; mv.z += right.z; }
                    if i.key_down(egui::Key::A) { mv.x -= right.x; mv.z -= right.z; }
                    if i.key_down(egui::Key::E) { mv.y += 1.0; }
                    if i.key_down(egui::Key::Q) { mv.y -= 1.0; }
                    (mv, i.stable_dt.min(0.05), i.modifiers.shift)
                });
                if mv.x != 0.0 || mv.y != 0.0 || mv.z != 0.0 {
                    let speed = (if boost { 42.0 } else { 16.0 }) * dt;
                    self.orbit.target.x += mv.x * speed;
                    self.orbit.target.y += mv.y * speed;
                    self.orbit.target.z += mv.z * speed;
                    self.spatial_auto = false; self.spatial_zoomed = true;
                }
            }
            ui.ctx().request_repaint();
        }

        // VOICE DIRECTIONAL TRAVEL: "move back"/"move forward"/… drift the camera continuously along the
        // look basis until "stop"/"hold here" clears it. Same basis as freecam, but hands-free and steady.
        if let Some(dir) = self.cam_drift {
            use crate::spatial::Vec3 as V3;
            use crate::voice_intent::MoveDir;
            let (cy, sy) = (self.orbit.yaw.cos(), self.orbit.yaw.sin());
            let (cp, sp) = (self.orbit.pitch.cos(), self.orbit.pitch.sin());
            let fwd = V3::new(cp * sy, -sp, -cp * cy);   // eye→target
            let right = V3::new(cy, 0.0, sy);
            let v = match dir {
                MoveDir::Forward => fwd,
                MoveDir::Back => V3::new(-fwd.x, -fwd.y, -fwd.z),
                MoveDir::Right => right,
                MoveDir::Left => V3::new(-right.x, 0.0, -right.z),
                MoveDir::Up => V3::new(0.0, 1.0, 0.0),
                MoveDir::Down => V3::new(0.0, -1.0, 0.0),
            };
            let dt = ui.input(|i| i.stable_dt).min(0.05);
            self.walk_ramp += (1.0 - self.walk_ramp) * (dt * 3.5).min(1.0); // ease the drift up to full over ~0.3s
            let speed = 14.0 * dt * self.walk_ramp;
            self.orbit.target.x += v.x * speed;
            self.orbit.target.y += v.y * speed;
            self.orbit.target.z += v.z * speed;
            self.spatial_auto = false;
            ui.ctx().request_repaint();
        }

        // WALKING BOB — a gentle footstep cadence on the view while drifting HORIZONTALLY (not up/down), so a
        // voice "walk"/"start moving" reads as walking rather than a dead-steady glide. The bob is eased in/out
        // and applied as a per-frame DELTA, so it never accumulates into a permanent vertical drift.
        {
            let dt = ui.input(|i| i.stable_dt).min(0.05);
            let walking = self.cam_drift.map_or(false, |d| !matches!(d, crate::voice_intent::MoveDir::Up | crate::voice_intent::MoveDir::Down));
            let want = if walking { self.walk_phase += dt * 7.2; self.walk_phase.sin() * 0.05 } else { 0.0 };
            let eased = self.walk_bob + (want - self.walk_bob) * (dt * 6.0).min(1.0);
            self.orbit.target.y += eased - self.walk_bob;
            self.walk_bob = eased;
            if walking { ui.ctx().request_repaint(); }
        }
        // path (parent links via LCA) — Catmull-Rom through the node positions — instead of jumping straight there.
        if let Some(t0) = self.cam_pan_t0 {
            let np = self.cam_pan_path.len();
            if np >= 2 {
                use crate::spatial::Vec3 as V3;
                let pp: Vec<V3> = self.cam_pan_path.iter().map(|&id| pos[id.min(pos.len() - 1)]).collect();
                let dur = 0.55 + 0.16 * (np as f32 - 1.0);          // longer, smoother glide along the path
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
            let auto = self.fly_auto.or_else(|| std::env::var("LOGSCOPE_FLY_AUTO").ok().and_then(|v| v.trim().parse::<f32>().ok()));
            let manual = std::env::var("LOGSCOPE_FLY").ok().and_then(|v| v.trim().parse::<f32>().ok());
            let s_opt = if let Some(spd) = auto {
                // Variable-speed traversal: advance the strand position by dt scaled DOWN by the current node's
                // dwell weight, so longer messages linger and short ones pass quicker (bounded, see tour_dwell).
                // Falls back to the old time-linear mapping if dwell weights aren't computed yet.
                if !self.tour_dwell.is_empty() && n > 1 {
                    let dt = ui.input(|i| i.stable_dt).min(0.05);
                    let cur = (self.tour_s.clamp(0.0, 0.999) * (n - 1) as f32).round() as usize;
                    let dwell = self.tour_dwell.get(cur).copied().unwrap_or(1.0);
                    self.tour_s = (self.tour_s + dt * spd * 0.02 / dwell).fract();
                    Some(self.tour_s)
                } else {
                    let tt = ui.input(|i| i.time) as f32;
                    Some(((tt * spd * 0.015).fract() + 1.0).fract())
                }
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
                let look = cr(pm, p1, p2, p3, (fr + 0.34).min(1.0)); // look well ahead → a SMOOTH tangent that doesn't whip at every node
                let (tx, ty, tz) = (look.x - here.x, look.y - here.y, look.z - here.z);
                let tl = (tx * tx + ty * ty + tz * tz).sqrt().max(1e-3);
                let (txn, tyn, tzn) = (tx / tl, ty / tl, tz / tl);
                let base_pitch = ((-tyn).asin().clamp(-1.2, 1.2)) + 0.12; // downward bias to see floor/pearls
                let base_yaw = txn.atan2(-tzn);
                let base_dist = std::env::var("LOGSCOPE_FLY_DIST").ok().and_then(|d| d.trim().parse().ok()).unwrap_or(5.2);
                // ── CINEMATIC DIRECTOR ── a shot state machine fed the graph's AABB (centroid + bounding radius)
                // and the current strand point. It cycles slowly through filmic shots — travelling along the strand,
                // pulling back to ESTABLISH the whole graph (the re-frame on arrival), slow ORBIT arcs, DUTCH tilts,
                // and close PUSH-IN passes — with slow eases so the camera glides like a movie rather than snapping.
                let now = ui.input(|i| i.time) as f32;
                let cc = crate::spatial::centroid(&pos);
                let fitd = crate::spatial::fit_distance(crate::spatial::bounding_radius(&pos, cc));
                let rnd = |seed: f32| ((seed * 12.9898).sin() * 43758.5453).fract().abs();
                if now >= self.cam_shot_until {
                    let h = rnd(now);
                    self.cam_shot = match self.cam_shot {                         // arrival → often ESTABLISH (re-frame graph)
                        0 => if h < 0.5 { 1 } else if h < 0.75 { 2 } else { 3 },  // travel → establish / orbit / dutch
                        1 => if h < 0.6 { 0 } else { 2 },                         // establish → travel / orbit
                        2 => if h < 0.55 { 0 } else if h < 0.8 { 1 } else { 4 },  // orbit → travel / establish / push
                        3 => 0,                                                   // dutch → travel
                        _ => if h < 0.6 { 0 } else { 1 },                         // push → travel / establish
                    };
                    self.cam_shot_until = now + 11.0 + h * 8.0;                   // 11–19 s dwell: fewer cuts, slower film pacing
                }
                self.cam_orbit_phase += 0.0026;                                  // slow continuous drift for orbit/dutch
                let (mut want_target, mut want_yaw, mut want_pitch, mut want_dist, mut want_roll, mut k) =
                    (here, base_yaw, base_pitch, base_dist, 0.0f32, 0.02f32);     // default: slow travel along the strand
                match self.cam_shot {
                    1 => { want_target = cc; want_dist = fitd; want_pitch = 0.42; want_yaw = base_yaw + (self.cam_orbit_phase * 0.4).sin() * 0.5; k = 0.014; } // ESTABLISH
                    2 => { want_target = here; want_dist = (fitd * 0.55).clamp(7.0, 40.0); want_pitch = 0.3; want_yaw = self.cam_orbit_phase * 0.8; k = 0.018; } // ORBIT arc
                    3 => { want_roll = (self.cam_orbit_phase * 0.5).sin() * 0.26; k = 0.022; }                                                                   // DUTCH tilt
                    4 => { want_dist = 4.4; want_pitch = base_pitch * 0.7; k = 0.026; }                                                                          // PUSH-IN passthrough (less extreme)
                    _ => {}
                }
                // GUARD: a collapsed graph (bounding_radius→0) or a gigantic one can make fit_distance return
                // NaN/inf or an absurd distance; a degenerate spline point can make want_target non-finite. Any
                // of these propagate into orbit.dist/target → a NaN eye → NaN projection → a BLANK/broken frame
                // (this is the "starting the tour breaks the render" bug). Clamp them to sane, finite values here.
                if !want_dist.is_finite() { want_dist = base_dist; }
                want_dist = want_dist.clamp(2.0, 240.0);
                if !(want_target.x.is_finite() && want_target.y.is_finite() && want_target.z.is_finite()) { want_target = here; }
                if !want_yaw.is_finite() { want_yaw = base_yaw; }
                if !want_pitch.is_finite() { want_pitch = base_pitch; }
                self.orbit.target.x += (want_target.x - self.orbit.target.x) * k;
                self.orbit.target.y += (want_target.y - self.orbit.target.y) * k;
                self.orbit.target.z += (want_target.z - self.orbit.target.z) * k;
                let mut dyaw = want_yaw - self.orbit.yaw;
                while dyaw > std::f32::consts::PI { dyaw -= std::f32::consts::TAU; }
                while dyaw < -std::f32::consts::PI { dyaw += std::f32::consts::TAU; }
                self.orbit.yaw += dyaw * k;
                self.orbit.pitch += (want_pitch - self.orbit.pitch) * k;
                self.orbit.dist += (want_dist - self.orbit.dist) * k;
                self.orbit.roll += (want_roll - self.orbit.roll) * (k * 0.8);    // ease the Dutch tilt in/out
                // final belt-and-braces: never let the eased orbit carry a non-finite value into projection
                if !self.orbit.dist.is_finite() { self.orbit.dist = base_dist; }
                self.orbit.dist = self.orbit.dist.clamp(2.0, 240.0);
                if !self.orbit.yaw.is_finite() { self.orbit.yaw = base_yaw; }
                if !self.orbit.pitch.is_finite() { self.orbit.pitch = base_pitch; }
                if !(self.orbit.target.x.is_finite() && self.orbit.target.y.is_finite() && self.orbit.target.z.is_finite()) { self.orbit.target = here; }

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
                // Horizontal palace AABB from the same colliders (Pillars cover hallway + wing geometry, Bound the
                // rotunda perimeter) → a hard OUTER bound for the fly, below.
                let (mut ax0, mut az0, mut ax1, mut az1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for c in crate::scene3d::env_colliders() {
                    match c {
                        crate::scene3d::Collider::Pillar { x, z, r, y0, y1 } => {
                            floor_y = floor_y.min(y0); ceil_y = ceil_y.max(y1);
                            ax0 = ax0.min(x - r); ax1 = ax1.max(x + r); az0 = az0.min(z - r); az1 = az1.max(z + r);
                        }
                        crate::scene3d::Collider::Bound { r } => {
                            ax0 = ax0.min(-r); ax1 = ax1.max(r); az0 = az0.min(-r); az1 = az1.max(r);
                        }
                        _ => {}
                    }
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
                // COLLIDER-INDEPENDENT HEIGHT CAP: even when a scene emits no floor/ceiling colliders (so the
                // clamp above is skipped) or the dome is very tall, never let the eye climb more than a bounded
                // vantage above/below the graph itself. Without this, a big-graph ESTABLISH pull-back sends the
                // eye straight into the sky — the "start tour flies up very high" bug. Generous (2.5× the
                // bounding radius) so it still frames the whole palace from above, just not from orbit.
                {
                    let br = crate::spatial::bounding_radius(&pos, cc).max(4.0);
                    let ey = eye.y + py;
                    let hi = cc.y + br * 2.5 + 6.0;
                    let lo = cc.y - br * 2.5 - 6.0;
                    if ey > hi { py += hi - ey; } else if ey < lo { py += lo - ey; }
                }
                // HARD OUTER BOUND: clamp the eye into the palace AABB (+margin) so bounded traversal holds even
                // when the fly slips out a gate down a hall — it stops at the hall's end instead of sailing into
                // open ocean (pass-63 overshoot). Bound = union of the node extent (so the eye can always reach
                // every strand node and pull back to ESTABLISH them) and the collider solids (walls/wings), so the
                // clamp never fights the spline; it only catches what would leave the palace.
                let (mut bx0, mut bz0, mut bx1, mut bz1) = (ax0, az0, ax1, az1);
                for q in &pos { bx0 = bx0.min(q.x); bx1 = bx1.max(q.x); bz0 = bz0.min(q.z); bz1 = bz1.max(q.z); }
                // Skip the outer clamp during the ESTABLISH (1) and ORBIT (2) shots — those DELIBERATELY pull
                // the eye far outside the graph extent to frame the whole palace. Clamping them jammed the eye
                // onto the graph edge, so the scene fell entirely out of view ("starting tour blanks the
                // scene"). The close travel/dutch/push shots still get the clamp, catching a slip out a hallway
                // into open ocean (the pass-63 overshoot it was added for).
                let framing_shot = matches!(self.cam_shot, 1 | 2);
                if bx0.is_finite() && !framing_shot {
                    let m = 4.0;
                    let ex = eye.x + px;
                    if ex < bx0 - m { px += (bx0 - m) - ex; } else if ex > bx1 + m { px += (bx1 + m) - ex; }
                    let ez = eye.z + pz;
                    if ez < bz0 - m { pz += (bz0 - m) - ez; } else if ez > bz1 + m { pz += (bz1 + m) - ez; }
                }
                // EASE the evasion offset toward the freshly-computed push so the camera SLIDES around columns and
                // along walls instead of snapping (the snap is what read as "ricochet"). The smoothed offset is
                // what shifts the eye+target, preserving look direction.
                let target_push = crate::spatial::Vec3::new(px, py, pz);
                self.fly_push.x += (target_push.x - self.fly_push.x) * 0.18;
                self.fly_push.y += (target_push.y - self.fly_push.y) * 0.18;
                self.fly_push.z += (target_push.z - self.fly_push.z) * 0.18;
                self.orbit.target.x += self.fly_push.x; self.orbit.target.y += self.fly_push.y; self.orbit.target.z += self.fly_push.z;

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
                // GUARD: a node landing on/behind the camera plane (e.g. the tour's close push-in) can project to
                // a non-finite depth; if it enters `proj` it poisons near/far/span via the fold, turning every
                // depth-normalized colour factor into NaN → Color32 multiply panics ("tour breaks the render").
                if sx.is_finite() && sy.is_finite() && depth.is_finite() {
                    proj.push((nd.id, sx, sy, depth));
                }
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
            let cord_col = if beam { egui::Color32::from_rgb(0xb6, 0xee, 0xff) } else if name == "Canopy" { egui::Color32::from_rgb(0xd8, 0xe4, 0xe8) } else if name == "Frozen Bifrost" { egui::Color32::from_rgb(0x9a, 0xb0, 0xe0) } else { edge_col };
            let mut graph: Vec<m3::Mesh3> = Vec::with_capacity(n * 2);
            let tanim = text_anim_for(name); // per-scene text-animation idiom for all scene/UI labels
            let sheen = slab_sheen(tanim, t); // per-scene light-play multiplier for physical UI slabs
            let sun = m3::sun_dir_3d(); // world sun direction, for specular glints on physical slabs
            // VOICE LIVE-INTENT HIGHLIGHT: while an utterance is being heard, parse the running PARTIAL — if it
            // already resolves to an ACTIONABLE command (not plain dictation) we light up the mic (below), and
            // for "go to N" we glow the target node, so the user sees what will happen before whisper finalizes.
            // A debug env var lets the headless harness inject a partial for render verification.
            let voice_partial = self.voice.as_ref().map(|v| v.partial().to_string())
                .filter(|s| !s.trim().is_empty())
                .or_else(|| std::env::var("LOGSCOPE_VOICE_PARTIAL").ok().filter(|s| !s.trim().is_empty()));
            let (voice_actionable, voice_target): (bool, Option<usize>) = match voice_partial.as_deref().map(crate::voice_intent::parse) {
                None | Some(crate::voice_intent::VoiceIntent::Dictate(_)) => (false, None),
                Some(crate::voice_intent::VoiceIntent::Command(crate::vimmode::Command::Goto(n))) => (true, Some(n.saturating_sub(1))),
                Some(_) => (true, None),
            };
            for nd in &g.nodes {
                if let Some(pid) = nd.parent {
                    let mut st = m3::with_mat(m3::strand(pos[pid], pos[nd.id], cord_r, 6, cord_seg, t, cord_amp, cord_col), strand_mat);
                    st.emissive = cord_glow;
                    if name == "Canopy" { m3::set_alpha(&mut st, 85); } // faint, see-through air
                    if name == "Frozen Bifrost" { m3::set_alpha(&mut st, 74); st.emissive = 0.6; } // faint glowing constellation lines between stars
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
                } else if name == "Frozen Bifrost" {
                    // ASGARD: each turn is a STAR in the sky off the world — a small bright emissive point (the
                    // twinkle is applied to emissive below), rendered in the space void the scene sets up.
                    m3::with_mat(m3::uv_sphere(pos[nd.id], 0.15, 10, 8, col), node_mat)
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
                nm.emissive = if rune { 0.40 + 0.10 * (t * 1.3 + nd.id as f32 * 0.5).sin() }
                    else if name == "Frozen Bifrost" { 2.2 + 0.9 * (t * 2.0 + nd.id as f32 * 1.3).sin() } // stars twinkle
                    else { pearl_glow };
                if active { nm.emissive = 0.9 + 0.35 * (t * 1.6).sin(); }
                let vhi = voice_target == Some(nd.id);
                if vhi { nm.emissive = 1.0 + 0.5 * (t * 6.0).sin(); } // the node the user is asking for — pulse hot
                graph.push(nm);
                if vhi {
                    // a bright accent RING around the spoken-of node, so "go to twelve" visibly marks node 12 live
                    let a = theme::accent();
                    let mut ring = m3::with_mat(m3::torus(pos[nd.id], 0.44, 0.03, 20, 6, a), node_mat);
                    ring.emissive = 1.4 + 0.6 * (t * 6.0).sin(); graph.push(ring);
                }
                // ASGARD: cross-shaped GLINT rays off each star, so the nodes read as sparkling points of light.
                if name == "Frozen Bifrost" {
                    let p = pos[nd.id];
                    let gl = 0.34 + 0.12 * (t * 2.0 + nd.id as f32).sin();
                    for (dx, dz) in [(gl, 0.0), (0.0, gl)] {
                        let a = crate::spatial::Vec3::new(p.x - dx, p.y, p.z - dz);
                        let b = crate::spatial::Vec3::new(p.x + dx, p.y, p.z + dz);
                        let mut ray = m3::with_mat(m3::tube(a, b, 0.012, 3, col), node_mat);
                        ray.emissive = 1.6; graph.push(ray);
                    }
                    // a faint vertical glint too (a 4-point star)
                    let mut vray = m3::with_mat(m3::tube(crate::spatial::Vec3::new(p.x, p.y - gl, p.z), crate::spatial::Vec3::new(p.x, p.y + gl, p.z), 0.012, 3, col), node_mat);
                    vray.emissive = 1.6; graph.push(vray);
                }
                // Treehouse: each wood ball HANGS from the overhead canopy — a sagging rope rising to a canopy
                // anchor KNOT (varied height, so ropes reach real branch level), with a couple of splay web
                // strands hooking the anchor into the surrounding foliage (not a stub ending in mid-air).
                if treehouse {
                    let p = pos[nd.id];
                    let canopy_y = p.y + 3.4 + 0.6 * (nd.id as f32 * 1.7).sin();
                    let anchor = crate::spatial::Vec3::new(p.x + 0.3 * (nd.id as f32).sin(), canopy_y, p.z + 0.3 * (nd.id as f32).cos());
                    let rope_col = egui::Color32::from_rgb(0x46, 0x34, 0x20);
                    let mut rope = m3::with_mat(m3::strand(anchor, p, 0.022, 4, 6, t, 0.10, rope_col), crate::texgen::mat::WOOD);
                    if active { rope.emissive = 0.2; }
                    graph.push(rope);
                    let knot = m3::with_mat(m3::uv_sphere(anchor, 0.09, 6, 5, egui::Color32::from_rgb(0x3a, 0x2a, 0x18)), crate::texgen::mat::WOOD);
                    graph.push(knot);
                    for wi in 0..2 { // splay webs hooking the anchor into the canopy branches
                        let wa = wi as f32 * std::f32::consts::PI + nd.id as f32;
                        let out = crate::spatial::Vec3::new(anchor.x + wa.cos() * 0.9, anchor.y + 0.45, anchor.z + wa.sin() * 0.9);
                        graph.push(m3::with_mat(m3::tube(anchor, out, 0.012, 3, rope_col), crate::texgen::mat::WOOD));
                    }
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
                                                    0.034, t, theme::CYAN(), lit, scroll, tanim, m3::SceneLight::Uniform, 0) {
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
                // scale a palette colour into an [u8;3] — used by the derived fallback below to tint the
                // plaque/accent from the active palette rather than a fixed grey.
                let scl = |c: egui::Color32, k: f32| -> [u8; 3] {
                    [(c.r() as f32 * k).clamp(0.0, 255.0) as u8,
                     (c.g() as f32 * k).clamp(0.0, 255.0) as u8,
                     (c.b() as f32 * k).clamp(0.0, 255.0) as u8]
                };
                let (pc, pmat, pemis, palpha, tc, tglow): ([u8; 3], f32, f32, u8, [u8; 3], f32) = match name {
                    "Atlantis"     => ([0xe6, 0xec, 0xf2], crate::texgen::mat::WATER, 0.28, 118, [0x9c, 0xee, 0xff], 1.25),
                    "Canopy"       => ([0x86, 0xb8, 0x52], crate::texgen::mat::MOSS,  0.14, 122, [0xe8, 0xf6, 0x9a], 1.15), // backlit leaf glow
                    "Mistfall"     => ([0x46, 0x60, 0x78], crate::texgen::mat::STONE, 0.08, 158, [0x66, 0xc6, 0xff], 1.0),  // carved blue rune
                    "Archive"      => ([0xe6, 0xd8, 0xbc], crate::texgen::mat::PAPER, 0.04, 188, [0x4a, 0x32, 0x1c], 0.45), // ink on parchment
                    "Reading Room" => ([0xf0, 0xe8, 0xd6], crate::texgen::mat::PAPER, 0.04, 190, [0x2e, 0x24, 0x18], 0.42), // typewriter ink
                    "Mansion Garden" => ([0xf0, 0xe6, 0xcc], crate::texgen::mat::STONE, 0.14, 150, [0xd8, 0xb4, 0x5c], 0.9), // cream marble + gold veins
                    // Every OTHER world derives a coherent idiom from its active palette (material + warm
                    // light + cool accent) instead of the old flat-grey quad, so the message panel and the
                    // mic pearl read as belonging to the scene. Dark worlds glow their accent on a dim tinted
                    // plaque; light worlds ink on a pale surface of their own warm light. This makes the
                    // panel idiom — and thus the mic pearl derived from it — cover all scenes for free.
                    _ => {
                        let pal = theme::active();
                        let (pmat, pemis, palpha) = match pal.material {
                            theme::Material::Paper  => (crate::texgen::mat::PAPER, 0.04, 186),
                            theme::Material::Wood   => (crate::texgen::mat::WOOD,  0.12, 150),
                            theme::Material::Marble => (crate::texgen::mat::STONE, 0.16, 140),
                            _                       => (crate::texgen::mat::STONE, 0.10, 138), // Stone / None
                        };
                        if pal.dark {
                            (scl(pal.light, 0.30), pmat, pemis, palpha, [pal.cyan.r(), pal.cyan.g(), pal.cyan.b()], 1.0)
                        } else {
                            (scl(pal.light, 0.96), pmat, (pemis * 0.6).min(0.06), palpha.max(180), [pal.ink.r(), pal.ink.g(), pal.ink.b()], 0.5)
                        }
                    }
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
                // WINDOW-SIZE AWARENESS: canvas dims + the local screen scale (px per world unit at `cen`'s
                // depth), so the physical UI can be ANCHORED to screen regions and SIZED to canvas fractions
                // (via `screen_to_world` / `ppw`) instead of fixed world offsets that drift off small windows.
                let (win_w, win_h) = (rect.width(), rect.height());
                let (ppw_x, ppw_y) = px_per_world(&view_cam, cen, right, up, cx, cy, scale);
                let ppw = ppw_x.min(ppw_y).max(1.0);
                let anchor = |ax: f32, ay: f32| screen_to_world(&view_cam, cen, right, up, cx, cy, scale, rect.left() + ax * win_w, rect.top() + ay * win_h);
                // IN-SCENE PANEL BOX removed by default: the app is moving to a separate message box + text-entry
                // box, so this third boxy plaque is redundant (and some of its layers weren't rendering cleanly).
                // LOGSCOPE_PANEL=1 restores it. The MIC pearl (voice state) + node/voice highlights stay — they're
                // not part of the box.
                // RETIRED as the composer: superseded by the pearl composer further down (reuses
                // pearl_input's proven cell-pool rendering, anchored/scaled the same way as the dial panel
                // and reading box, instead of this block's fixed world offsets — which is exactly what was
                // drifting it off-frame on other aspect ratios). Kept opt-in for comparison only.
                let draw_panel = std::env::var("LOGSCOPE_PANEL").ok().as_deref() == Some("1");
                if draw_panel {
                // DROP SHADOW — a larger, dark, soft quad set further back and offset down-right (as if lit
                // from the upper-left), so the physical UI tablet casts a shadow and reads with real DEPTH
                // instead of floating flat against the scene.
                let scen = V3::new(cen.x + fwd.x * 0.16 + right.x * 0.10 - up.x * 0.10,
                                   cen.y + fwd.y * 0.16 + right.y * 0.10 - up.y * 0.10,
                                   cen.z + fwd.z * 0.16 + right.z * 0.10 - up.z * 0.10);
                let mut shadow = m3::panel(scen, right, up, 4.55, 1.26, egui::Color32::from_rgb(0, 0, 0));
                shadow.mat = crate::texgen::mat::STONE; shadow.emissive = 0.0; m3::set_alpha(&mut shadow, 96);
                graph.push(shadow);
                // bezel frame just BEHIND the plaque (darker shade of the panel, a touch more opaque) so the
                // message entry reads as a physical tablet/plaque rather than a floating quad.
                let bez = if name == "Atlantis" { egui::Color32::from_rgb(0x1c, 0x5c, 0x6a) } else { egui::Color32::from_rgb((pc[0] as f32 * 0.5) as u8, (pc[1] as f32 * 0.5) as u8, (pc[2] as f32 * 0.5) as u8) };
                let fcen = V3::new(cen.x + fwd.x * 0.05, cen.y + fwd.y * 0.05, cen.z + fwd.z * 0.05);
                let mut frame = m3::panel(fcen, right, up, 4.3, 1.06, bez);
                frame.mat = pmat; frame.emissive = if name == "Atlantis" { 0.34 } else { pemis * 0.4 }; m3::set_alpha(&mut frame, (palpha as u16 + 50).min(255) as u8);
                graph.push(frame);
                // a thin glowing accent strip down the left edge, in the scene's text colour — a small UI cue.
                // It flares to the theme ACCENT and pulses when the live voice partial is already an actionable
                // command, so the physical panel itself signals "heard — this will do something".
                let acen = V3::new(cen.x - right.x * 1.92, cen.y - right.y * 1.92, cen.z - right.z * 1.92);
                let (astrip_col, astrip_emis, astrip_alpha) = if voice_actionable {
                    (theme::accent(), 0.9 + 0.5 * (t * 6.0).sin(), 255u8)
                } else {
                    (egui::Color32::from_rgb(tc[0], tc[1], tc[2]), 0.45 + pemis, 224u8)
                };
                let mut accent = m3::panel(acen, right, up, 0.08, 0.66, astrip_col);
                accent.mat = pmat; accent.emissive = astrip_emis; m3::set_alpha(&mut accent, astrip_alpha);
                graph.push(accent);
                let mut panel = m3::panel(cen, right, up, 4.0, 0.82, egui::Color32::from_rgb(pc[0], pc[1], pc[2]));
                panel.mat = pmat; panel.emissive = pemis;
                m3::set_alpha(&mut panel, palpha);
                graph.push(panel);
                // ACCESSIBILITY / INTERACTION OVERLAY — the message panel is a real 3D object; lay a transparent
                // egui widget over its projected screen rect so the pointer (hover cursor) and screen-reader
                // (AccessKit) both target the physical slab in the scene. Sense::hover so it never steals clicks
                // from the scene behind it. This is the framework for bringing UI controls into the world as
                // interactive 3D assets (project the slab → overlay an egui widget on its rect).
                if let Some(prect) = project_slab_rect(&view_cam, cen, right, up, 4.0, 0.82, cx, cy, scale) {
                    let ov = ui.interact(prect, egui::Id::new("scene_msg_panel"), egui::Sense::click())
                        .on_hover_text("Message panel — click to type here, or speak into the pearl");
                    if ov.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::Text); }
                    // click the physical slab → focus the composer's text input, so the in-scene panel is a real
                    // interactive control (the framework for moving the composer fully into the world).
                    if ov.clicked() {
                        ui.ctx().memory_mut(|m| m.request_focus(egui::Id::new("logscope_chat_composer")));
                    }
                }
                let typing = !self.input.trim().is_empty();
                let shown: String = if typing {
                    let nch = self.input.chars().count();
                    self.input.chars().skip(nch.saturating_sub(20)).collect()
                } else { "speak into the pearl".chars().take(20).collect() };
                let mut lit = (if typing { 0.95_f32 } else { 0.34 }) * tglow;
                let mut tcol = egui::Color32::from_rgb(tc[0], tc[1], tc[2]);
                if name == "Frozen Bifrost" { // ASGARD: the panel text reads as a CONSTELLATION of starlight glyph-points
                    tcol = egui::Color32::from_rgb(0xdc, 0xe6, 0xff); lit = lit.max(0.55) + 0.35;
                }
                // LEFT-ALIGN from a fixed left edge so every glyph sits in a STABLE pixel column as the prompt
                // grows (label_cells centres on its origin, which would slide the text around as you type). Font
                // is ~12% smaller than before so longer prompts fit. This is what sells "typing into the panel".
                let tscale = 0.030_f32;
                let nshown = shown.trim().chars().count().min(32) as f32;
                let tw = nshown * 6.0 * tscale;
                let lx = 1.86_f32; // half the text field width (panel half is 2.0)
                let bx = V3::new(cen.x - right.x * lx, cen.y - right.y * lx, cen.z - right.z * lx); // fixed left edge
                let tcen = V3::new(bx.x + right.x * (tw * 0.5) + up.x * 0.06, bx.y + right.y * (tw * 0.5) + up.y * 0.06, bx.z + right.z * (tw * 0.5) + up.z * 0.06);
                for cell in m3::label_cells(tcen, right, up, toward, shown.trim(), tscale, t, tcol, lit, 0.0, tanim, m3::SceneLight::Uniform, 0) {
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
                } // end draw_panel (in-scene panel box)
                // MIC PEARL — a small marble-sheen orb beside the message panel, showing live voice state at a
                // glance. Deliberately a DIFFERENT physical object from the flat resonance-cell text (a glossy
                // stone/pearl bead rather than glyph cells), but its colour is DERIVED from the same per-scene
                // (pc, tc, tglow, pmat) table above — so every scene's mic pearl is a free, coherent derivative
                // of its message-panel idiom rather than a bespoke style per world.
                //   asleep (mic off)   — dim grey mote, barely breathing.
                //   armed (mic on)     — a warm marble shell, slow breathing glow in the scene's accent colour.
                //   active (hearing an utterance) — brighter + faster pulse + two orbiting sparkle motes, so
                //     "it's catching your voice" reads even before whisper finalizes the utterance.
                // armed/active state: real self.voice, or LOGSCOPE_MIC_PEARL_DEBUG=armed|active for headless
                // render verification (mic-pearl-view bin) — the `voice` feature can't build without a real
                // ALSA/whisper.cpp toolchain, so this is how the pearl's states get visually checked per scene.
                let debug = std::env::var("LOGSCOPE_MIC_PEARL_DEBUG").ok();
                let armed = self.voice.is_some() || matches!(debug.as_deref(), Some("armed") | Some("active"));
                let active = self.voice.as_ref().map(|v| !v.partial().is_empty()).unwrap_or(false) || debug.as_deref() == Some("active");
                let mcen = V3::new(cen.x + right.x * 2.22 + fwd.x * 0.06, cen.y + up.y * 0.02 + fwd.y * 0.06, cen.z + right.z * 2.22 + fwd.z * 0.06);
                let breathe = if active { (t * 3.4).sin() * 0.5 + 0.5 } else { (t * 1.1).sin() * 0.5 + 0.5 };
                // LOGSCOPE_MIC_PEARL_SCALE (default 1.0) enlarges the pearl for per-scene SHAPE verification —
                // the ship bead is small (its shape reads as a silhouette/material cue), so this debug hook
                // (paired with LOGSCOPE_MIC_PEARL_DEBUG) lets the headless harness show the gem/pearl/stone/bead
                // geometry clearly across scenes without affecting the shipped size.
                let mscale = std::env::var("LOGSCOPE_MIC_PEARL_SCALE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0).clamp(0.3, 12.0);
                let r = (0.12 + (if active { 0.045 } else { 0.018 }) * breathe) * mscale;
                let shell = if voice_actionable {
                    theme::accent() // an actionable command is already recognized in the live partial → flash accent
                } else if armed {
                    egui::Color32::from_rgb((pc[0] as u16 + 18).min(255) as u8, (pc[1] as u16 + 14).min(255) as u8, (pc[2] as u16 + 6).min(255) as u8)
                } else { egui::Color32::from_rgb(0x6a, 0x6a, 0x70) }; // asleep: dim grey mote, no scene tint
                // SHAPE is the scene's physical substance (a second derivative beyond colour) — it holds
                // across all states, so an asleep gem still reads as a gem. A stable per-scene seed varies the
                // gem facets / stone lumps between worlds. Colour/material/emissive still track the mic state.
                let mshape = mic_shape(name, theme::active().material);
                let mseed = name.bytes().fold(0x811C_9DC5u32, |a, b| (a ^ b as u32).wrapping_mul(0x0100_0193));
                let mut orb = match mshape {
                    MicShape::Gem   => m3::cracked_pearl(mcen, r, mseed, shell), // faceted iridescent crystal
                    MicShape::Stone => m3::rock(mcen, r, mseed, shell),          // weathered / carved stone bead
                    MicShape::Bead  => m3::uv_sphere(mcen, r, 7, 5, shell),      // chunky low-poly turned wood bead
                    MicShape::Leaf  => m3::leaf_single(mcen, r * 1.35, mseed, shell), // backlit foliage leaf blade
                    MicShape::Pearl => m3::uv_sphere(mcen, r, 10, 8, shell),     // smooth pearl / paper wafer
                };
                orb.mat = if !armed { crate::texgen::mat::STONE }
                          else if mshape == MicShape::Leaf { crate::texgen::mat::MOSS } // leaf reads as translucent foliage
                          else { pmat };
                orb.emissive = if voice_actionable { (0.9 + 0.7 * breathe) * tglow } else if !armed { 0.06 } else if active { (0.55 + 0.85 * breathe) * tglow } else { (0.18 + 0.22 * breathe) * tglow };
                graph.push(orb);
                // SCENE-RELEVANT SETTING — the pearl sits IN something that belongs to the world (shell / gem
                // cradle / rune plinth / wooden dish / dew stem), so the voice indicator reads as a placed
                // asset, not a primitive floating in space. Dim + matte so the glowing orb still leads.
                let shape_code = match mshape { MicShape::Pearl => 0, MicShape::Gem => 1, MicShape::Stone => 2, MicShape::Bead => 3, MicShape::Leaf => 4 };
                for m in crate::scene3d::mic_setting(name, shape_code, mcen, r, right, up, fwd, mseed, egui::Color32::from_rgb(pc[0], pc[1], pc[2])) {
                    graph.push(m);
                }
                // ACCESSIBILITY OVERLAY on the mic object — the second in-scene UI control exposed to pointer +
                // screen-reader, labelled by voice state. Sense::hover so it never steals scene clicks; a click
                // handler will toggle voice once that backend is wired.
                if let Some(mrect) = project_slab_rect(&view_cam, mcen, right, up, r * 2.4, r * 2.4, cx, cy, scale) {
                    let label = if active { "Voice mic — listening" } else if armed { "Voice mic — armed" } else { "Voice mic — off" };
                    let mo = ui.interact(mrect, egui::Id::new("scene_mic_object"), egui::Sense::hover()).on_hover_text(label);
                    if mo.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                }
                if active {
                    // two tiny orbiting sparkle motes in the scene's accent colour — the pearl actively
                    // "catching" your voice, distinct from the steady breathing glow of just being armed.
                    for i in 0..2u32 {
                        let ang = t * 2.2 + i as f32 * std::f32::consts::PI;
                        let orad = r * 2.1;
                        let ox = mcen.x + right.x * ang.cos() * orad + up.x * ang.sin() * orad * 0.6;
                        let oy = mcen.y + right.y * ang.cos() * orad + up.y * ang.sin() * orad * 0.6;
                        let oz = mcen.z + right.z * ang.cos() * orad + up.z * ang.sin() * orad * 0.6;
                        let mut mote = m3::uv_sphere(V3::new(ox, oy, oz), r * 0.22, 5, 4, egui::Color32::from_rgb(tc[0], tc[1], tc[2]));
                        mote.emissive = 1.6 * tglow;
                        graph.push(mote);
                    }
                }

                // ── PHYSICAL CONTROL DOCK ────────────────────────────────────────────────────────
                // Overview / Tour / Freecam as in-scene physical BUTTONS (real 3D objects you press),
                // driven by the tested `physui` FSM. `dock_arc` lays them on a shallow arc below the
                // pearl; `project_slab_rect` hit-tests each; `ControlAnim` eases the press/hover/lit
                // feel; and each actuation drives the SAME action as the 2D toolbar (which stays — so
                // nothing that works today breaks). LOGSCOPE_PHYS_DOCK=0 disables it (A/B + safety valve).
                if !world_only
                    && std::env::var("LOGSCOPE_PHYS_DOCK").map(|v| v != "0").unwrap_or(true)
                {
                    use crate::physui::Pointer;
                    // Keep the physical toggles in step with state changed elsewhere (2D button, voice
                    // command) BEFORE reading input, so a press starts from the true lit state.
                    self.phys.set_active(DOCK_TOUR, self.fly_auto.is_some());
                    self.phys.set_active(DOCK_FREECAM, self.freecam);

                    // dt for the eased button feel (clamped so a long stall doesn't jump the ease).
                    let now = std::time::Instant::now();
                    let dt = self.phys_t.map(|p| (now - p).as_secs_f32()).unwrap_or(1.0 / 60.0).clamp(0.0, 0.1);
                    self.phys_t = Some(now);

                    // Lay the dock on a shallow arc just below the message pearl, curving toward the
                    // viewer; billboarded slabs (right/up basis) so each button faces the camera. The
                    // small downward offset keeps it a ledge beneath the pearl without dropping off the
                    // bottom of the canvas (the panel origin already sits low-centre).
                    let dock_c = anchor(0.5, 0.87); // bottom-centre, adapts to window size
                    let dock_radius = (0.42 * win_w / ppw / 1.36).clamp(0.55, 1.7); // arc spans ~42% of width
                    let ds = dock_radius / 0.95; // scale buttons with the arc
                    let pos = crate::physui::dock_arc(
                        DOCK_IDS.len(),
                        [dock_c.x, dock_c.y, dock_c.z],
                        [right.x, right.y, right.z],
                        [toward.x, toward.y, toward.z], // arc bulges toward the user
                        dock_radius, 1.5,
                    );
                    let (bw, bh) = (0.29_f32 * ds, 0.26_f32 * ds); // button slab size (world units)
                    // Prunable churn (matches the 2D "Prune churn (N)" button): superseded turns not yet pruned.
                    let sup = g.superseded();
                    let prunable = sup.iter().filter(|id| !self.pruned.contains(id)).count();

                    // Project each button to a screen rect (hit-test target + a11y overlay bounds).
                    let mut rects: Vec<(usize, egui::Rect)> = Vec::with_capacity(DOCK_IDS.len());
                    for (i, &id) in DOCK_IDS.iter().enumerate() {
                        let bc = V3::new(pos[i][0], pos[i][1], pos[i][2]);
                        if let Some(rct) = project_slab_rect(&view_cam, bc, right, up, bw * 1.2, bh * 1.5, cx, cy, scale) {
                            rects.push((id, rct));
                        }
                    }

                    // Drive the physui FSM from the raw pointer: which control is under the pointer, and
                    // did the primary button go down / up this frame. Actuation happens on a release over
                    // the SAME control the press started on (drag-off cancels) — the FSM enforces that;
                    // here we just feed it. Raw pointer state, so it's independent of egui click routing.
                    let ptr = ui.input(|i| i.pointer.interact_pos());
                    let over = ptr.and_then(|pp| rects.iter().find(|(_, r)| r.contains(pp)).map(|(id, _)| *id));
                    let (down, up_ev) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released()));
                    let ev = if down { self.phys.on_pointer(Pointer::Down, over) }
                             else if up_ev { self.phys.on_pointer(Pointer::Up, over) }
                             else { self.phys.on_pointer(Pointer::Move, over) };

                    // Route an actuation to the SAME field mutations as the 2D toolbar buttons.
                    if let Some(act) = ev.and_then(dock_action) {
                        // kick the actuation flash on the control that fired (so the press visibly *does* something)
                        let fired = match ev { Some(crate::physui::ControlEvent::Fired(i)) | Some(crate::physui::ControlEvent::Toggled(i, _)) => Some(i), _ => None };
                        if let Some(fid) = fired { if let Some(ix) = DOCK_IDS.iter().position(|&d| d == fid) { self.phys_anim[ix].kick_flash(); } }
                        match act {
                            DockAction::Overview => { self.spatial_auto = true; self.spatial_zoomed = false; self.walk_focus = None; }
                            DockAction::Tour(on) => {
                                self.fly_auto = if on { Some(1.0) } else { None };
                                if on { self.freecam = false; } else { self.spatial_auto = true; self.spatial_zoomed = false; }
                            }
                            DockAction::Freecam(on) => {
                                self.freecam = on;
                                if on { self.fly_auto = None; self.spatial_auto = false; self.spatial_zoomed = true; }
                                else { self.spatial_auto = true; self.spatial_zoomed = false; }
                            }
                            DockAction::Prune => { for id in &sup { self.pruned.insert(*id); } } // no-op when nothing superseded
                            DockAction::Unprune => { self.pruned.clear(); } // restore every pruned turn
                        }
                        ui.ctx().request_repaint();
                    }
                    // Re-sync the toggles so THIS frame's anim reads the just-applied lit state.
                    self.phys.set_active(DOCK_TOUR, self.fly_auto.is_some());
                    self.phys.set_active(DOCK_FREECAM, self.freecam);

                    // Draw each button eased by its ControlAnim: hover brightens, press pushes the slab
                    // IN (toward −toward, away from the eye), active lights it. Each face carries a short
                    // glowing LABEL in its control colour (cyan Overview · amber Tour · green Freecam ·
                    // rose Prune), so the dock reads without hovering. Prune dims when there's nothing to
                    // prune. State-aware labels: TOUR↔STOP, FREE↔EXIT.
                    let pip = [
                        egui::Color32::from_rgb(0x7a, 0xe0, 0xff), // Overview — cyan
                        egui::Color32::from_rgb(0xff, 0xc2, 0x6a), // Tour — amber
                        egui::Color32::from_rgb(0x8e, 0xe6, 0x9a), // Freecam — green
                        egui::Color32::from_rgb(0xff, 0x92, 0x9a), // Prune — rose
                        egui::Color32::from_rgb(0xc9, 0xa8, 0xff), // Unprune — lavender
                    ];
                    let face = [
                        "VIEW",
                        if self.fly_auto.is_some() { "STOP" } else { "TOUR" },
                        if self.freecam { "EXIT" } else { "FREE" },
                        "PRUNE",
                        "UNDO",
                    ];
                    let mut animating = false;
                    for (i, &id) in DOCK_IDS.iter().enumerate() {
                        let vis = self.phys.visual(id);
                        self.phys_anim[i].tick(vis, dt);
                        // DEBUG (headless verification of the tactile visuals): force control #i into a held
                        // press + flash so a render shows the depression, flash ring, and label pop.
                        if std::env::var("LOGSCOPE_PHYS_FORCE").ok().and_then(|s| s.trim().parse::<usize>().ok()) == Some(i) {
                            self.phys_anim[i].press = 1.0; self.phys_anim[i].hover = 1.0;
                            self.phys_anim[i].flash = self.phys_anim[i].flash.max(0.7);
                        }
                        let a = self.phys_anim[i];
                        if a.flash > 0.002 || (a.press.abs() > 0.002 && !vis.pressed) || a.hover > 0.002 { animating = true; }
                        // Prune enabled only when there's churn to drop; Unprune only when something is pruned.
                        let enabled = match id {
                            DOCK_PRUNE => prunable > 0,
                            DOCK_UNPRUNE => !self.pruned.is_empty(),
                            _ => true,
                        };
                        // springy press: pushes IN when pressed, pops slightly PROUD (press<0) on release.
                        let push = a.press * 0.07;
                        let bc = V3::new(pos[i][0] - toward.x * push, pos[i][1] - toward.y * push, pos[i][2] - toward.z * push);
                        let base = egui::Color32::from_rgb(pc[0], pc[1], pc[2]);
                        let mut btn = m3::panel(bc, right, up, bw, bh, base);
                        btn.mat = pmat;
                        let dim = if enabled { 1.0 } else { 0.4 };
                        // emissive: base + hover + active + a bright ACTUATION FLASH burst
                        btn.emissive = (0.12 + 0.55 * a.hover + 0.85 * a.active + 1.5 * a.flash) * tglow * dim * sheen;
                        m3::set_alpha(&mut btn, if enabled { 220 } else { 150 });
                        graph.push(btn); slab_glint(&mut graph, bc, right, up, toward, bw * 0.5, bh * 0.5, sun, tglow);
                        // actuation flash RING — a thin frame that flares just outside the slab and fades, so a
                        // press reads as a real event even at a glance.
                        if a.flash > 0.02 {
                            let g = 1.0 + 0.28 * a.flash; // expands as it fires
                            let mut ring = m3::panel(bc, right, up, bw * g + 0.06, bh * g + 0.06, pip[i]);
                            ring.mat = pmat;
                            ring.emissive = 2.2 * a.flash * tglow.max(0.6);
                            m3::set_alpha(&mut ring, (150.0 * a.flash) as u8);
                            graph.push(ring);
                        }
                        // glowing face label, floated just proud of the slab. A change/press makes it POP
                        // (scale + brightness) AND slide into place (scroll) via the flash, so text visibly
                        // reacts — a TOUR→STOP swap reads as motion, not an instant flip.
                        let lbl = face[i];
                        let lscale = (bw * 0.86 / (lbl.len() as f32 * 6.0)).min(0.017) * (1.0 + 0.14 * a.flash);
                        let lcen = V3::new(bc.x - toward.x * 0.30, bc.y - toward.y * 0.30, bc.z - toward.z * 0.30);
                        let llit = if enabled { (0.55 + 0.5 * a.hover + 0.7 * a.active + 0.8 * a.flash).min(1.6) } else { 0.18 };
                        for cell in m3::label_cells(lcen, right, up, toward, lbl, lscale, t, pip[i], llit, a.flash * 0.06, tanim, m3::SceneLight::Uniform, 0) {
                            graph.push(cell);
                        }
                    }
                    if animating { ui.ctx().request_repaint(); }

                    // Accessibility / interaction overlay: a transparent egui widget over each button's
                    // projected rect gives the pointer (hand cursor) and screen-reader (AccessKit) a real
                    // target, and — being added after the scene canvas — keeps a button press from also
                    // orbiting the camera. Labels track the toggle state.
                    for &id in DOCK_IDS.iter() {
                        if let Some((_, rct)) = rects.iter().find(|(rid, _)| *rid == id) {
                            let label = match id {
                                DOCK_OVERVIEW => "Overview — recenter and fit the whole graph".to_string(),
                                DOCK_TOUR => if self.fly_auto.is_some() { "Stop tour".into() } else { "Tour — fly through the palace".into() },
                                DOCK_FREECAM => if self.freecam { "Exit freecam".into() } else { "Freecam — free-flight camera".into() },
                                DOCK_PRUNE => if prunable > 0 { format!("Prune churn ({prunable}) — drop superseded turns to free context") } else { "Prune churn — nothing to prune".into() },
                                DOCK_UNPRUNE => if !self.pruned.is_empty() { format!("Unprune — restore all {} pruned turns to context", self.pruned.len()) } else { "Unprune — nothing pruned".into() },
                                _ => String::new(),
                            };
                            let resp = ui.interact(*rct, egui::Id::new(("scene_phys_ctrl", id)), egui::Sense::click())
                                .on_hover_text(label);
                            if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                        }
                    }

                    // ── PHYSICAL SETTINGS PANEL — the Context panel's sampling sliders as physical DIALS ──
                    // A plaque on the left of the scene, one draggable dial per self.ctx param (temperature, ctx
                    // window, reply budget, repeat penalty, min-p). Two-way bound: the handle tracks the live value
                    // (so 2D edits reflect) and dragging writes back to self.ctx. (The old time dial is gone —
                    // the scene tracks system time.) LOGSCOPE_PHYS_PARAMS=0 hides it.
                    if std::env::var("LOGSCOPE_PHYS_PARAMS").map(|v| v != "0").unwrap_or(true) {
                        let panel_c = anchor(0.15, 0.60); // lower-left corner, adapts to window size
                        let plaque_w = (0.24 * win_w / ppw).clamp(0.9, 2.6);
                        let ps = plaque_w / 1.42; // scale the tuned layout by the window-derived plaque width
                        let back = |o: f32| V3::new(panel_c.x - toward.x * o, panel_c.y - toward.y * o, panel_c.z - toward.z * o);
                        // backing plaque (set back so the sliders sit proud of it)
                        let mut plaque = m3::panel(back(-0.06 * ps), right, up, 1.42 * ps, 1.60 * ps, egui::Color32::from_rgb(pc[0], pc[1], pc[2]));
                        plaque.mat = pmat; plaque.emissive = 0.07 * tglow * sheen; m3::set_alpha(&mut plaque, 150);
                        graph.push(plaque); slab_glint(&mut graph, back(-0.06 * ps), right, up, toward, 0.71 * ps, 0.80 * ps, sun, tglow);
                        let title_c = V3::new(panel_c.x - toward.x * 0.30 + up.x * 0.66 * ps, panel_c.y - toward.y * 0.30 + up.y * 0.66 * ps, panel_c.z - toward.z * 0.30 + up.z * 0.66 * ps);
                        for cell in m3::label_cells(title_c, right, up, toward, "PARAMS", 0.010 * ps, t, egui::Color32::from_rgb(232, 238, 252), 0.7, 0.0, tanim, m3::SceneLight::Uniform, 0) { graph.push(cell); }

                        let (p_down, p_pressed) = ui.input(|i| (i.pointer.primary_down(), i.pointer.primary_pressed()));
                        let p_pos = ui.input(|i| i.pointer.interact_pos());
                        if !p_down { self.slider_drag = None; }
                        let (tw, th) = (0.64_f32 * ps, 0.038_f32 * ps);
                        for (row, &(id, label, lo, hi, isint)) in PARAM_DIALS.iter().enumerate() {
                            let off = (0.42 - row as f32 * 0.26) * ps;
                            let rc = V3::new(panel_c.x + up.x * off, panel_c.y + up.y * off, panel_c.z + up.z * off);
                            let track_rect = project_slab_rect(&view_cam, rc, right, up, tw, th * 3.2, cx, cy, scale);
                            if let Some(tr) = track_rect {
                                if p_pressed && p_pos.map_or(false, |p| tr.contains(p)) { self.slider_drag = p_pos.map(|p| (id, p.x)); }
                            }
                            let dragging_this = matches!(self.slider_drag, Some((d, _)) if d == id);
                            if dragging_this {
                                if let (Some((_, last_x)), Some(p), Some(tr)) = (self.slider_drag, p_pos, track_rect) {
                                    let dx = p.x - last_x;
                                    if dx.abs() > 0.01 {
                                        let frac = dx / tr.width().max(1.0);
                                        if let Some(crate::physui::ControlEvent::Adjusted(_, v)) = self.phys.drag(id, frac) { ctx_set(&mut self.ctx, id, v); }
                                        self.slider_drag = Some((id, p.x));
                                        ui.ctx().request_repaint();
                                    }
                                }
                            } else {
                                // handle tracks the live value when not being dragged
                                self.phys.set_value(id, ctx_get(&self.ctx, id));
                            }
                            let val = self.phys.value_of(id);
                            let frac = ((val - lo) / (hi - lo)).clamp(0.0, 1.0);
                            let mut trk = m3::panel(rc, right, up, tw, th, egui::Color32::from_rgb(pc[0], pc[1], pc[2]));
                            trk.mat = pmat; trk.emissive = 0.12 * tglow * sheen; m3::set_alpha(&mut trk, 185);
                            graph.push(trk);
                            let hx = (frac - 0.5) * tw;
                            let hc = V3::new(rc.x + right.x * hx, rc.y + right.y * hx, rc.z + right.z * hx);
                            let mut handle = m3::uv_sphere(hc, th * 1.7, 8, 6, egui::Color32::from_rgb(0x8a, 0xe0, 0xff));
                            handle.mat = pmat; handle.emissive = (0.5 + if dragging_this { 0.9 } else { 0.35 }) * tglow.max(0.6);
                            graph.push(handle);
                            // label to the left, value to the right (floated proud of the track)
                            let lx = -(tw * 0.5 + 0.20 * ps);
                            let lcen = V3::new(rc.x - toward.x * 0.30 + right.x * lx + up.x * 0.11 * ps, rc.y - toward.y * 0.30 + right.y * lx + up.y * 0.11 * ps, rc.z - toward.z * 0.30 + right.z * lx + up.z * 0.11 * ps);
                            for cell in m3::label_cells(lcen, right, up, toward, label, 0.010 * ps, t, egui::Color32::from_rgb(206, 214, 234), 0.6, 0.0, tanim, m3::SceneLight::Uniform, 0) { graph.push(cell); }
                            let vtxt = if isint { format!("{}", val.round() as i32) } else { format!("{val:.2}") };
                            let vx = tw * 0.5 + 0.10 * ps;
                            let vcen = V3::new(rc.x - toward.x * 0.30 + right.x * vx + up.x * 0.11 * ps, rc.y - toward.y * 0.30 + right.y * vx + up.y * 0.11 * ps, rc.z - toward.z * 0.30 + right.z * vx + up.z * 0.11 * ps);
                            for cell in m3::label_cells(vcen, right, up, toward, &vtxt, 0.010 * ps, t, egui::Color32::from_rgb(0x8a, 0xe0, 0xff), 0.72, 0.0, tanim, m3::SceneLight::Uniform, 0) { graph.push(cell); }
                            if let Some(tr) = track_rect {
                                let hint = format!("{label}: {vtxt} — drag to change");
                                let resp = ui.interact(tr, egui::Id::new(("scene_phys_param", id)), egui::Sense::click_and_drag()).on_hover_text(hint);
                                if resp.hovered() || dragging_this { ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal); }
                            }
                        }

                        // ── SELECTOR ROWS — backend / persona / model, the popout's remaining controls,
                        // as physical cycle-plaques below the dials. A click advances to the next option
                        // (physui::ControlKind::Selector, wrapping); the label shows the CURRENT option so
                        // the whole thing reads as a labeled dial rather than a dropdown pretending to be 3D.
                        self.phys.set_index(BACKEND_SEL, if self.backend == BackendKind::ALL[0] { 0 } else { 1 });
                        let persona_idx = PERSONAS.iter().position(|(_, _, body)| self.context.trim() == body.trim()).unwrap_or(usize::MAX);
                        if persona_idx != usize::MAX { self.phys.set_index(PERSONA_SEL, persona_idx); }
                        if let Some(m) = model.as_deref_mut() {
                            self.phys.set_selector_count(MODEL_SEL, m.items.len());
                            self.phys.set_index(MODEL_SEL, m.selected);
                        }
                        let sel_rows: &[(usize, &str)] = &[(BACKEND_SEL, "BACKEND"), (PERSONA_SEL, "PERSONA"), (MODEL_SEL, "MODEL")];
                        for (row, &(id, label)) in sel_rows.iter().enumerate() {
                            if id == MODEL_SEL && model.is_none() { continue; } // no model context (headless/backdrop scene)
                            let off = (0.42 - (PARAM_DIALS.len() + row) as f32 * 0.26) * ps;
                            let rc = V3::new(panel_c.x + up.x * off, panel_c.y + up.y * off, panel_c.z + up.z * off);
                            let track_rect = project_slab_rect(&view_cam, rc, right, up, tw, th * 3.2, cx, cy, scale);
                            let cur = match id {
                                BACKEND_SEL => BackendKind::ALL[self.phys.index_of(id).min(1)].label().to_string(),
                                PERSONA_SEL => PERSONAS.get(self.phys.index_of(id)).map(|(_, title, _)| title.to_string()).unwrap_or_default(),
                                MODEL_SEL => model.as_deref().and_then(|m| m.items.get(m.selected)).map(|it| it.name().to_string()).unwrap_or_else(|| "—".into()),
                                _ => String::new(),
                            };
                            let mut trk = m3::panel(rc, right, up, tw, th, egui::Color32::from_rgb(pc[0], pc[1], pc[2]));
                            trk.mat = pmat; trk.emissive = 0.12 * tglow * sheen; m3::set_alpha(&mut trk, 185);
                            graph.push(trk);
                            let lx = -(tw * 0.5 + 0.20 * ps);
                            let lcen = V3::new(rc.x - toward.x * 0.30 + right.x * lx + up.x * 0.11 * ps, rc.y - toward.y * 0.30 + right.y * lx + up.y * 0.11 * ps, rc.z - toward.z * 0.30 + right.z * lx + up.z * 0.11 * ps);
                            for cell in m3::label_cells(lcen, right, up, toward, label, 0.010 * ps, t, egui::Color32::from_rgb(206, 214, 234), 0.6, 0.0, tanim, m3::SceneLight::Uniform, 0) { graph.push(cell); }
                            let vcen = V3::new(rc.x - toward.x * 0.30 + up.x * 0.11 * ps, rc.y - toward.y * 0.30 + up.y * 0.11 * ps, rc.z - toward.z * 0.30 + up.z * 0.11 * ps);
                            for cell in m3::label_cells(vcen, right, up, toward, &cur, 0.010 * ps, t, egui::Color32::from_rgb(0x8a, 0xe0, 0xff), 0.62, 0.0, tanim, m3::SceneLight::Uniform, 0) { graph.push(cell); }
                            if let Some(tr) = track_rect {
                                let resp = ui.interact(tr, egui::Id::new(("scene_phys_sel", id)), egui::Sense::click()).on_hover_text(format!("{label}: {cur} — click to cycle"));
                                if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                                if resp.clicked() {
                                    if let Some(crate::physui::ControlEvent::Selected(_, idx)) = self.phys.nudge_selector(id, 1) {
                                        match id {
                                            BACKEND_SEL => { self.backend = BackendKind::ALL[idx.min(1)]; }
                                            PERSONA_SEL => { if let Some((_, _, body)) = PERSONAS.get(idx) { self.context = (*body).to_string(); } }
                                            MODEL_SEL => { if let Some(m) = model.as_deref_mut() { if idx < m.items.len() { m.selected = idx; } } }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ── IN-SCENE COMPOSER — the pearl, projected from a 3D-anchored slab ────────────────
                // Only needed when the 2D bottom bar is hidden (hide_bottom_bar): that's the toggle that
                // currently has no physical replacement, leaving no way to type at all in full-screen
                // spatial mode. Reuses pearl_input's proven cell-pool rendering via show_at(rect) — sized
                // from win_w/win_h/ppw like the dial panel and reading box above, instead of the old
                // draw_panel's fixed world offsets that put it below the visible frame on other aspect
                // ratios/window sizes. Only the text field + Enter-to-send are ported here; the fuller
                // attach/mic/export row stays 2D-only for now (toggle the bar back on to reach those).
                if self.hide_bottom_bar {
                    let comp_c = anchor(0.5, 0.90); // bottom-centre, adapts to window size
                    let comp_w = (0.62 * win_w / ppw).clamp(2.2, 6.5);
                    let comp_h = (0.20 * win_h / ppw).clamp(0.55, 1.3);
                    if let Some(rect) = project_slab_rect(&view_cam, comp_c, right, up, comp_w, comp_h, cx, cy, scale) {
                        let pearl_t = ui.input(|i| i.time) as f32;
                        let vpartial = self.voice.as_ref().map(|v| v.partial().to_string()).unwrap_or_default();
                        // scene accent (the panel idiom's text colour) so the composer pearl belongs to
                        // THIS world — Atlantis prismarine · Canopy leaf-glow · Mistfall rune-blue · Archive
                        // ink — rather than a fixed cyan. Same (tc) the mic pearl and reading plaque use.
                        let comp_accent = egui::Color32::from_rgb(tc[0], tc[1], tc[2]);
                        let band = self.pearl_input.show_at(ui, rect, &self.input, &vpartial, pearl_t, comp_accent);
                        ui.visuals_mut().text_cursor.stroke = egui::Stroke::NONE;
                        ui.visuals_mut().selection.bg_fill = egui::Color32::from_rgba_unmultiplied(0x4d, 0xd6, 0xe6, 36);
                        let resp = ui.put(band, egui::TextEdit::multiline(&mut self.input)
                            .id(egui::Id::new("logscope_chat_composer"))
                            .desired_width(f32::INFINITY)
                            .frame(egui::Frame::NONE)
                            .text_color(egui::Color32::TRANSPARENT)
                            .font(egui::FontId::new(14.0, egui::FontFamily::Monospace)));
                        let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                        if enter && self.input.ends_with('\n') { self.input.pop(); }
                        if resp.changed() {
                            self.mode = crate::vimmode::Mode::Insert;
                            if !resp.has_focus() { resp.request_focus(); }
                        }
                        if enter && !self.input.trim().is_empty() {
                            self.pearl_input.pulse();
                            send_now = true;
                        }

                        // ── PHYSICAL ACTION ROW — Attach / Mic / Send as in-scene slabs, mirroring the 2D
                        // composer's buttons so full-screen spatial mode (bottom bar hidden) can still attach,
                        // dictate, and send. A small three-button ledge just under the composer pearl.
                        let miced = self.voice.is_some();
                        let acts: [(&str, &str); 3] = [
                            ("ATT", "attach a text file"),
                            (if miced { "REC" } else { "MIC" }, if miced { "stop mic" } else { "dictate" }),
                            ("SEND", "send"),
                        ];
                        let arow_c = anchor(0.5, 0.985); // just below the composer
                        let btn_w = (0.08 * win_w / ppw).clamp(0.35, 0.9);
                        let gap = btn_w * 1.35;
                        for (i, (glyph, hint)) in acts.iter().enumerate() {
                            let dx = (i as f32 - 1.0) * gap; // centre the middle button
                            let bc = V3::new(arow_c.x + right.x * dx, arow_c.y + right.y * dx, arow_c.z + right.z * dx);
                            let mut btn = m3::panel(bc, right, up, btn_w, btn_w * 0.8, egui::Color32::from_rgb(pc[0], pc[1], pc[2]));
                            btn.mat = pmat; btn.emissive = 0.14 * tglow * sheen; m3::set_alpha(&mut btn, 200);
                            graph.push(btn);
                            let gcen = V3::new(bc.x - toward.x * 0.30, bc.y - toward.y * 0.30, bc.z - toward.z * 0.30);
                            for cell in m3::label_cells(gcen, right, up, toward, glyph, 0.013 * (btn_w / 0.6), t, egui::Color32::from_rgb(tc[0], tc[1], tc[2]), 0.8, 0.0, tanim, m3::SceneLight::Uniform, 0) { graph.push(cell); }
                            if let Some(tr) = project_slab_rect(&view_cam, bc, right, up, btn_w * 1.15, btn_w * 0.95, cx, cy, scale) {
                                let resp = ui.interact(tr, egui::Id::new(("scene_action_btn", i)), egui::Sense::click()).on_hover_text(*hint);
                                if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                                if resp.clicked() {
                                    match i {
                                        0 => { // attach
                                            let start = self.save_dir.clone()
                                                .or_else(|| std::env::var("HOME").ok().map(std::path::PathBuf::from))
                                                .unwrap_or_else(|| std::path::PathBuf::from("/"));
                                            self.attach_picker.open_file(&start, "Attach a file", &[]);
                                        }
                                        1 => { // mic toggle
                                            if self.voice.is_some() { self.voice = None; }
                                            else {
                                                match crate::voice_session::VoiceSession::start() {
                                                    Ok(s) => { self.voice = Some(s); self.voice_err = None; ui.ctx().request_repaint(); }
                                                    Err(e) => { self.voice_err = Some(format!("voice: {e}")); }
                                                }
                                            }
                                        }
                                        _ => { // send
                                            if !self.input.trim().is_empty() { self.pearl_input.pulse(); send_now = true; }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ── AGENT PLAN — the committed step list (2), rendered as a checklist that ticks off as
                // the run progresses, so the agent's intent is visible and you can steer/interrupt. Sits
                // just above the workspace-files feed on the right edge. Only when a plan exists.
                if !self.backdrop && !self.agent_plan.is_empty() {
                    let pc0 = anchor(0.86, 0.13);
                    let pw = (0.22 * win_w / ppw).clamp(0.8, 2.4);
                    let pscale = pw / 1.3;
                    let title_c = V3::new(pc0.x - toward.x * 0.30, pc0.y - toward.y * 0.30, pc0.z - toward.z * 0.30);
                    for cell in m3::label_cells(title_c, right, up, toward, "PLAN", 0.010 * pscale, t, egui::Color32::from_rgb(232, 238, 252), 0.7, 0.0, tanim, m3::SceneLight::Uniform, 0) { graph.push(cell); }
                    for (row, (text, done)) in self.agent_plan.iter().take(8).enumerate() {
                        let off = -(row as f32 + 1.0) * 0.15 * pscale;
                        let rc = V3::new(pc0.x + up.x * off - toward.x * 0.30, pc0.y + up.y * off - toward.y * 0.30, pc0.z + up.z * off - toward.z * 0.30);
                        let mark = if *done { "[x] " } else { "[ ] " };
                        let col = if *done { egui::Color32::from_rgb(tc[0], tc[1], tc[2]) } else { egui::Color32::from_rgb(150, 160, 178) };
                        let line: String = format!("{mark}{}", text.chars().take(18).collect::<String>());
                        for cell in m3::label_cells(rc, right, up, toward, &line, 0.008 * pscale, t, col, if *done { 0.75 } else { 0.5 }, 0.0, tanim, m3::SceneLight::Uniform, 0) { graph.push(cell); }
                    }
                }

                // ── WORKSPACE FILES — live feed of files the agent wrote / you dropped, so file creation
                // is VISIBLE in the scene (the ask: "see file creation"). Right-edge plaque, newest last,
                // colour-coded (created = accent, modified = dim). Only when a session has touched files.
                if !self.backdrop && !self.agent_files.is_empty() {
                    let fc = anchor(0.86, 0.42);
                    let fw = (0.22 * win_w / ppw).clamp(0.8, 2.4);
                    let fs = fw / 1.3;
                    let back = |o: f32| V3::new(fc.x - toward.x * o, fc.y - toward.y * o, fc.z - toward.z * o);
                    let mut plaque = m3::panel(back(-0.06 * fs), right, up, 1.3 * fs, 1.7 * fs, egui::Color32::from_rgb(pc[0], pc[1], pc[2]));
                    plaque.mat = pmat; plaque.emissive = 0.07 * tglow * sheen; m3::set_alpha(&mut plaque, 150);
                    graph.push(plaque);
                    let title_c = V3::new(fc.x - toward.x * 0.30 + up.x * 0.72 * fs, fc.y - toward.y * 0.30 + up.y * 0.72 * fs, fc.z - toward.z * 0.30 + up.z * 0.72 * fs);
                    for cell in m3::label_cells(title_c, right, up, toward, "WORKSPACE", 0.010 * fs, t, egui::Color32::from_rgb(232, 238, 252), 0.7, 0.0, tanim, m3::SceneLight::Uniform, 0) { graph.push(cell); }
                    let n = self.agent_files.len();
                    let show = self.agent_files.iter().rev().take(8).collect::<Vec<_>>();
                    for (row, ev) in show.iter().enumerate() {
                        let off = (0.52 - row as f32 * 0.15) * fs;
                        let rc = V3::new(fc.x + up.x * off - toward.x * 0.30, fc.y + up.y * off - toward.y * 0.30, fc.z + up.z * off - toward.z * 0.30);
                        let created = matches!(ev.change, crate::workspace_exec::FileChange::Created);
                        let col = if created { egui::Color32::from_rgb(tc[0], tc[1], tc[2]) } else { egui::Color32::from_rgb(150, 160, 178) };
                        let mark = if created { "+ " } else { "~ " };
                        let name: String = ev.path.rsplit('/').next().unwrap_or(&ev.path).chars().take(16).collect();
                        for cell in m3::label_cells(rc, right, up, toward, &format!("{mark}{name}"), 0.008 * fs, t, col, if created { 0.75 } else { 0.5 }, 0.0, tanim, m3::SceneLight::Uniform, 0) { graph.push(cell); }
                    }
                    if n > 8 {
                        let mc = V3::new(fc.x + up.x * (0.52 - 8.0 * 0.15) * fs - toward.x * 0.30, fc.y + up.y * (0.52 - 8.0 * 0.15) * fs - toward.y * 0.30, fc.z + up.z * (0.52 - 8.0 * 0.15) * fs - toward.z * 0.30);
                        for cell in m3::label_cells(mc, right, up, toward, &format!("+{} more", n - 8), 0.008 * fs, t, egui::Color32::from_rgb(120, 130, 148), 0.5, 0.0, tanim, m3::SceneLight::Uniform, 0) { graph.push(cell); }
                    }
                }

                // ── CANDIDATES — the fan's folded branches as a clickable prune panel. Each card shows the
                // branch's review verdict (✓ approved / ~ verified / · plain), file count, and a short review
                // note; the reviewer-suggested branch is ringed. Click a card to LAND that branch (:pick).
                // This is the fold/prune decision made visual — the keeper is one click, not a typed command.
                if !self.backdrop && !self.agent_candidates.is_empty() {
                    let cc = anchor(0.17, 0.22);
                    let cw = (0.30 * win_w / ppw).clamp(1.4, 3.6);
                    let cs = cw / 2.0;
                    let best = self.agent_candidates.iter().max_by_key(|b| (b.approved == Some(true), b.verified, b.files)).map(|b| b.index);
                    let title_c = V3::new(cc.x - toward.x * 0.30 + up.x * 0.5 * cs, cc.y - toward.y * 0.30 + up.y * 0.5 * cs, cc.z - toward.z * 0.30 + up.z * 0.5 * cs);
                    for cell in m3::label_cells(title_c, right, up, toward, "PICK A BRANCH", 0.011 * cs, t, egui::Color32::from_rgb(232, 238, 252), 0.75, 0.0, tanim, m3::SceneLight::Uniform, 0) { graph.push(cell); }
                    let mut clicked_pick: Option<usize> = None;
                    let rowh = 0.30 * cs;
                    for (row, cand) in self.agent_candidates.iter().take(6).enumerate() {
                        let off = -(row as f32) * rowh;
                        let rc = V3::new(cc.x + up.x * off, cc.y + up.y * off, cc.z + up.z * off);
                        let is_best = best == Some(cand.index);
                        // card slab (accent-tinted for the suggested keeper)
                        let (cr, cg, cb) = if is_best { (tc[0], tc[1], tc[2]) } else { (pc[0], pc[1], pc[2]) };
                        let mut card = m3::panel(rc, right, up, cw, rowh * 0.82, egui::Color32::from_rgb(cr, cg, cb));
                        card.mat = pmat; card.emissive = if is_best { 0.28 } else { 0.12 } * tglow * sheen; m3::set_alpha(&mut card, if is_best { 210 } else { 170 });
                        graph.push(card);
                        // status glyph + label
                        let (mark, mcol) = match cand.approved {
                            Some(true) => ("[OK]", egui::Color32::from_rgb(140, 230, 150)),
                            Some(false) => ("[--]", egui::Color32::from_rgb(230, 150, 140)),
                            None if cand.verified => ("[~]", egui::Color32::from_rgb(tc[0], tc[1], tc[2])),
                            None => ("[ ]", egui::Color32::from_rgb(160, 170, 188)),
                        };
                        let head = format!("{mark} {} · {}f", cand.label.chars().take(14).collect::<String>(), cand.files);
                        let hc = V3::new(rc.x - toward.x * 0.30 + up.x * 0.06 * cs, rc.y - toward.y * 0.30 + up.y * 0.06 * cs, rc.z - toward.z * 0.30 + up.z * 0.06 * cs);
                        for cell in m3::label_cells(hc, right, up, toward, &head, 0.0085 * cs, t, mcol, 0.8, 0.0, tanim, m3::SceneLight::Uniform, 0) { graph.push(cell); }
                        // review note (one truncated line under the head)
                        if !cand.review_notes.is_empty() {
                            let note: String = cand.review_notes.lines().next().unwrap_or("").chars().take(28).collect();
                            let nc = V3::new(rc.x - toward.x * 0.30 - up.x * 0.06 * cs, rc.y - toward.y * 0.30 - up.y * 0.06 * cs, rc.z - toward.z * 0.30 - up.z * 0.06 * cs);
                            for cell in m3::label_cells(nc, right, up, toward, &note, 0.0072 * cs, t, egui::Color32::from_rgb(150, 160, 178), 0.55, 0.0, tanim, m3::SceneLight::Uniform, 0) { graph.push(cell); }
                        }
                        // click → land this branch
                        if let Some(tr) = project_slab_rect(&view_cam, rc, right, up, cw * 1.05, rowh * 0.9, cx, cy, scale) {
                            let hint = format!("branch {} · {} file(s){}{} — click to land it",
                                cand.index + 1, cand.files,
                                if cand.verified { " · verified" } else { "" },
                                match cand.approved { Some(true) => " · approved", Some(false) => " · needs revision", None => "" });
                            let resp = ui.interact(tr, egui::Id::new(("fan_candidate", cand.index)), egui::Sense::click()).on_hover_text(hint);
                            if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                            if resp.clicked() { clicked_pick = Some(cand.index + 1); }
                        }
                    }
                    if let Some(n) = clicked_pick { self.pick_branch(n); }
                }

                // ── MESSAGE READING BOX — the focused turn as a physical env-styled plaque ──────────
                // Clicking a node focuses it; its turn text then reads as a real in-world plaque (bezel + shadow
                // + slab + wrapped label_cells text) carrying the scene's palette/material/emissive — an object,
                // not a 2D overlay. (LOGSCOPE_FOCUS=<n> forces a focus for headless checks.)
                let focus = std::env::var("LOGSCOPE_FOCUS").ok().and_then(|s| s.trim().parse::<usize>().ok()).or(self.walk_focus);
                if let Some(fid) = focus.filter(|i| *i < self.msgs.len()) {
                    let (who, raw) = { let m = &self.msgs[fid]; (if m.role == Role::User { "you" } else { "assistant" }, m.text().to_string()) };
                    let head = format!("{who} · turn {}/{}", fid + 1, n);
                    // Reset the glide (don't inherit scroll position/animation) when focus moves to a
                    // DIFFERENT message this frame.
                    if self.plaque_last_focus != Some(fid) { self.plaque_scroll.reset(); self.plaque_last_focus = Some(fid); }
                    let all_lines = wrap_label(&raw, 24, usize::MAX);
                    let max_line = all_lines.len().saturating_sub(PLAQUE_WINDOW) as f32;
                    let dt = ui.input(|i| i.stable_dt).min(0.1); // clamp a stalled first frame's huge dt
                    let pos = self.plaque_scroll.tick(dt).clamp(0.0, max_line);
                    if !self.plaque_scroll.at_rest() { ui.ctx().request_repaint(); } // keep animating the glide
                    let start = pos.round() as usize;
                    let lines: Vec<String> = all_lines.iter().skip(start).take(PLAQUE_WINDOW).cloned().collect();
                    // scroll affordance: show "▲ n above" / "▼ n below" so scrolled state is legible in-scene.
                    let head = if all_lines.len() > PLAQUE_WINDOW {
                        let above = start; let below = all_lines.len().saturating_sub(start + lines.len());
                        let marks = match (above > 0, below > 0) {
                            (true, true) => " ▲▼", (true, false) => " ▲", (false, true) => " ▼", (false, false) => "",
                        };
                        format!("{head}{marks}  ⟨j/k to scroll⟩")
                    } else { head };
                    let rc = anchor(0.5, 0.17); // top-centre, adapts to window size
                    let pw = (0.44 * win_w / ppw).clamp(1.6, 5.2);
                    let ph = (pw * 0.42).min(0.30 * win_h / ppw); // cap by height so it fits short windows
                    let bscale = (pw / (24.0 * 6.0) * 0.90).clamp(0.006, 0.03); // body fits ~24 cols
                    let hscale = bscale * 0.82;
                    // drop shadow (offset down-right, set back) for real depth
                    let scen = V3::new(rc.x + fwd.x * 0.16 + right.x * 0.10 - up.x * 0.10, rc.y + fwd.y * 0.16 + right.y * 0.10 - up.y * 0.10, rc.z + fwd.z * 0.16 + right.z * 0.10 - up.z * 0.10);
                    let mut shadow = m3::panel(scen, right, up, pw + 0.5, ph + 0.4, egui::Color32::from_rgb(0, 0, 0));
                    shadow.mat = crate::texgen::mat::STONE; shadow.emissive = 0.0; m3::set_alpha(&mut shadow, 96); graph.push(shadow);
                    // bezel + slab in the scene palette/material
                    let bez = egui::Color32::from_rgb((pc[0] as f32 * 0.5) as u8, (pc[1] as f32 * 0.5) as u8, (pc[2] as f32 * 0.5) as u8);
                    let fcen = V3::new(rc.x + fwd.x * 0.05, rc.y + fwd.y * 0.05, rc.z + fwd.z * 0.05);
                    let mut frame = m3::panel(fcen, right, up, pw + 0.28, ph + 0.24, bez); frame.mat = pmat; frame.emissive = pemis * 0.4; m3::set_alpha(&mut frame, (palpha as u16 + 50).min(255) as u8); graph.push(frame);
                    let mut slab = m3::panel(rc, right, up, pw, ph, egui::Color32::from_rgb(pc[0], pc[1], pc[2])); slab.mat = pmat; slab.emissive = pemis * sheen; m3::set_alpha(&mut slab, palpha); graph.push(slab); slab_glint(&mut graph, rc, right, up, toward, pw * 0.5, ph * 0.5, sun, tglow);
                    // left-edge origin: label_cells centres on its origin, so offset each line by +half its width
                    let left_edge = pw * 0.5 - pw * 0.06;
                    // the plaque's text is lit by the CURRENT SCENE's physical light source (dappled
                    // canopy leaves, a rotating pearl's facets, …) rather than a flat dot-matrix glow —
                    // see mesh3d::SceneLight's doc comment for the full per-scene ledger. Seeded by the
                    // focused message id so different messages' motes/glints don't move in lockstep.
                    let scene_light = crate::mesh3d::scene_light_for(theme::active().name);
                    let plaque_seed = fid as u32;
                    let mut put_line = |g: &mut Vec<crate::mesh3d::Mesh3>, text: &str, y: f32, scale: f32, col: egui::Color32, lit: f32| {
                        let w = text.chars().count() as f32 * 6.0 * scale;
                        let o = V3::new(rc.x - toward.x * 0.30 - right.x * left_edge + right.x * (w * 0.5) + up.x * y,
                                        rc.y - toward.y * 0.30 - right.y * left_edge + right.y * (w * 0.5) + up.y * y,
                                        rc.z - toward.z * 0.30 - right.z * left_edge + right.z * (w * 0.5) + up.z * y);
                        for cell in m3::label_cells(o, right, up, toward, text, scale, t, col, lit, 0.0, tanim, scene_light, plaque_seed) { g.push(cell); }
                    };
                    put_line(&mut graph, &head, ph * 0.5 - ph * 0.13, hscale, egui::Color32::from_rgb(0xff, 0xc2, 0x6a), 0.85);
                    for (li, line) in lines.iter().enumerate() {
                        put_line(&mut graph, line, ph * 0.5 - ph * 0.38 - li as f32 * ph * 0.17, bscale, egui::Color32::from_rgb(tc[0], tc[1], tc[2]), 0.85 * tglow.max(0.5));
                    }
                    if let Some(pr) = project_slab_rect(&view_cam, rc, right, up, pw, ph, cx, cy, scale) {
                        ui.interact(pr, egui::Id::new("scene_reader"), egui::Sense::hover()).on_hover_text(format!("{head}: {}", raw.chars().take(160).collect::<String>()));
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
                let cam = crate::gpuscene::Camera::from_orbit(&view_cam, rect.width(), rect.height(), scale, 0.3, 400.0);
                let mut verts = Vec::new();
                // dynamic-LOD screen-size threshold (NDC) for SCENERY: drop meshes too small to read; more
                // aggressive under battery-saver POWER; tunable via LOGSCOPE_LOD, off via LOGSCOPE_NO_LOD=1.
                let lod_ndc = if std::env::var("LOGSCOPE_NO_LOD").map(|v| v == "1").unwrap_or(false) { 0.0 }
                    else { std::env::var("LOGSCOPE_LOD").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.0035) / crate::theme::power_mode().max(0.25) };
                crate::scene3d::with_env(name, t, |env| verts.extend(crate::gpuscene::pack_sorted(env, &cam, lod_ndc)));
                if !world_only { verts.extend(crate::gpuscene::pack_sorted(&graph, &cam, 0.0)); } // graph: no LOD (keep every node), drawn in front
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
                let caustic = match name { "Atlantis" => 0.32, "Mistfall" => 0.2, _ => 0.0 };
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
                        * match name { "Recursive Citadel" => 0.84, "Reading Room" => 0.9, "Folding@home" => 0.86, _ => 1.0 } // trim the over-bright worlds
                        * std::env::var("LOGSCOPE_EXP").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0),
                    saturation: std::env::var("LOGSCOPE_SAT").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.18),
                    shoulder: std::env::var("LOGSCOPE_SHOULDER").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0),
                    // per-scene BLOOM trim — Old Library + Cloudscape were over-blooming (detail washed out)
                    threshold: gthr * 1.0, bloom_amt: gbloom * 0.85 * match name { "Old Library" => 0.5, "Cloudscape" => 0.42, "Reading Room" => 0.4, "Recursive Citadel" => 0.5, "Folding@home" => 0.4, "Mansion Garden" => 0.26, _ => 1.0 }, texscale: dtexscale, bump: dbump,
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
            if self.walk_focus == Some(id) {
                // pulsing double halo so a voice/keyboard-focused node reads at a glance even mid-scene: an
                // outer ring that breathes in radius + a steady inner ring. (Was a single static ring.)
                let pulse = 0.5 + 0.5 * (time * 3.0 + id as f32).sin();
                p.circle_stroke(pt, radius + 3.0, egui::Stroke::new(2.2, theme::AMBER()));
                p.circle_stroke(pt, radius + 5.5 + 3.5 * pulse, egui::Stroke::new(1.6, theme::AMBER().linear_multiply(0.35 + 0.4 * (1.0 - pulse))));
            }
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

        // focused-node preview strip with prune/restore for that turn (skipped when used as a backdrop).
        // Floated as translucent glass anchored to the bottom of the scene — reserves no layout space.
        if !self.backdrop {
        let panel_w = ui.max_rect().width();
        egui::Area::new(egui::Id::new("scene_bottom_chrome"))
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(8.0, -8.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
            ui.set_max_width((panel_w - 16.0).max(160.0));
            glass_frame().show(ui, |ui| {
            if let Some(id) = self.walk_focus.filter(|i| *i < self.msgs.len()) {
                // The turn TEXT now reads on the physical plaque in the scene; this glass strip is just the
                // per-turn action bar (prune / restore / branch) for the focused node.
                let who = if self.msgs[id].role == Role::User { "you" } else { "assistant" };
                let prn = self.pruned.contains(&id);
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
            } else {
                ui.label(RichText::new("Click a node to read its turn here. ⊹ Overview reframes; ✂ prunes to free context.").color(theme::FAINT()).small());
            }
            }); });
        }
        send_now
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
            Command::Branch => { if let Some(f) = self.walk_focus { self.branch_from = Some(f); } }
            Command::Attach(p) => self.attach_path(&p, ui),
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
                let task = task.trim().to_string();
                if task.is_empty() {
                    self.note = ":agent needs a task (e.g. :agent add a unit test for the parser)".into();
                } else if self.gitmind_session.is_none() {
                    self.note = ":agent — load a workspace first (:load-gitmind <path>), then :agent <task>".into();
                } else if self.agent_run.is_some() {
                    self.note = ":agent — a run is already active (:stop to cancel it first)".into();
                } else if self.stream.is_some() {
                    self.note = ":agent — the model is busy; wait for the reply or :stop".into();
                } else {
                    // NON-BLOCKING: build the driver over the session's isolated /tmp workspace and hand its
                    // first prompt to the inference worker. The Evt loop (apply_evt) advances it turn by turn
                    // via on_token/on_done; the frame never blocks, and :stop / barge-in interrupt it.
                    let work_dir = self.gitmind_session.as_ref().unwrap().work_dir().to_path_buf();
                    let cfg = crate::workspace_exec::AgentConfig::default();
                    let (run, first) = crate::workspace_exec::AgentRun::start(work_dir, &task, cfg);
                    self.agent_run = Some(run);
                    self.agent_task = task.clone();
                    self.agent_pending_submit = Some(first);
                    self.agent_files.clear();
                    self.agent_plan.clear();
                    self.note = format!("agent started: {task} — planning…");
                }
            }
            Command::FanAgent(task) => {
                let cfg = crate::workspace_exec::AgentConfig::default();
                self.start_fan(task.trim(), cfg, false, "fan-agent");
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
                let cfg = crate::workspace_exec::AgentConfig::docs();
                self.start_fan(task.trim(), cfg, true, "docs");
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
                    if self.agent_run.take().is_some() || self.agent_fan.take().is_some() { self.agent_pending_submit = None; self.note = "agent run cancelled (barge-in)".into(); }
                }
            }
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

    pub fn ui(&mut self, ui: &mut egui::Ui, model: &mut ModelState, doc: &crate::analysis::Doc, infer: &Inference, model_loaded: bool) {
        self.sync_worker(infer);
        self.local_model_path = model.selected_local_path(); // for :lanes (run_command has no ModelState)
        self.pump_lanes(); // lane-pool events → fan (a second, parallel event source next to the worker)
        self.check_restore(); // first frame: offer to recover a crashed/closed conversation
        if self.pending_cancel { infer.cancel(); self.pending_cancel = false; }
        let streaming = self.stream.is_some();
        if streaming || self.agent_run.is_some() || self.agent_fan.is_some() || !self.lane_jobs.is_empty() { ui.ctx().request_repaint(); }
        self.pump_voice(infer, doc, ui, streaming); // mic → whisper → route utterances / barge-in (no-op when mic off)
        self.handle_modal(ui, infer, doc); // modal (vim) keys — both views; default Insert keeps typing free

        // ── top bar: status · threading toggle · context toggle · clear ──
        ui.horizontal(|ui| {
            let (badge, bc) = if model_loaded { ("model loaded", theme::GREEN()) } else { ("no model — Load a GGUF up top", theme::DIM()) };
            theme::chip(ui, badge, bc);
            ui.label(RichText::new(self.backend.label()).color(theme::FAINT()).small());
            if let Some(s) = &self.stream {
                let secs = s.started.elapsed().as_secs_f32().max(0.001);
                let toks = s.tokens;
                theme::pulse_dot(ui, if toks == 0 { theme::CYAN() } else { theme::AMBER() });
                let debug = std::env::var("LOGSCOPE_DEBUG").is_ok();
                if toks == 0 {
                    // prefill: the model is reading the prompt, no tokens yet (this is the slow part on big prompts)
                    ui.label(RichText::new(format!("thinking… ({:.0}s)", secs)).color(theme::CYAN()).small())
                        .on_hover_text("prefilling the prompt — the reply starts once this finishes");
                } else if debug {
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
                // auto-verify verdict of the just-generated Rust, surfaced HERE (not just the buried Files
                // panel) so it's obvious the model's code was actually build-checked.
                if let Some(job) = &self.verify_job {
                    match &job.result {
                        None => { theme::chip(ui, "⌗ checking build…", theme::AMBER()); }
                        Some((passed, output, _)) => match crate::sandbox::classify(*passed, output) {
                            crate::sandbox::Verdict::Pass => theme::chip(ui, "✓ build ok", theme::GREEN()),
                            crate::sandbox::Verdict::CompileError => theme::chip(ui, "✗ build failed", theme::RED()),
                            crate::sandbox::Verdict::TestFailure => theme::chip(ui, "✗ tests failed", theme::RED()),
                            crate::sandbox::Verdict::DepsUnavailable => theme::chip(ui, "⚠ deps not cached", theme::AMBER()),
                            crate::sandbox::Verdict::ToolError => theme::chip(ui, "⚠ verify error", theme::AMBER()),
                        },
                    }
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
                // Only meaningful in Spatial mode: the composer/files/suggestions strip otherwise
                // cramps or cuts off the physical UI's scene against the bottom of the window, with
                // no way to reclaim that space — this hides the WHOLE bottom panel, not just parts
                // of it, and moves nothing (no repositioning to "fit better") since a toggle is a
                // much more predictable fix than trying to auto-shrink composer sub-elements.
                if self.spatial_view {
                    ui.add_space(6.0);
                    let label = if self.hide_bottom_bar { "▲ Show bar" } else { "▼ Hide bar" };
                    if ui.selectable_label(self.hide_bottom_bar, RichText::new(label).color(theme::DIM()))
                        .on_hover_text("Hide the composer/files/suggestions strip entirely, so the physical UI gets the full window height instead of being cramped behind it.")
                        .clicked() {
                        self.hide_bottom_bar = !self.hide_bottom_bar;
                    }
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
                            // The physical PARAMS panel (dials + backend/persona/model selectors, in the
                            // spatial scene) is now the primary place for these controls. When it's active
                            // don't duplicate them here — a slider changed in two places at once is a bug
                            // waiting to happen. LOGSCOPE_PHYS_PARAMS=0 (or non-spatial view) falls back to
                            // this page as before.
                            let phys_params_on = self.spatial_view
                                && std::env::var("LOGSCOPE_PHYS_PARAMS").map(|v| v != "0").unwrap_or(true);
                            ui.label(RichText::new("CONTEXT (system prompt)").color(theme::DIM()).small().strong());
                            if phys_params_on {
                                ui.label(RichText::new("Sampling/backend/model moved into the scene — drag the PARAMS dials on the left, click BACKEND/PERSONA/MODEL to cycle them. Persona is still editable here.").color(theme::FAINT()).small());
                                ui.add_space(6.0);
                            }
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new("persona").color(theme::DIM()).small());
                                for (key, title, body) in PERSONAS {
                                    let active = self.context.trim() == body.trim();
                                    if ui.selectable_label(active, RichText::new(*title).small()).on_hover_text(*key).clicked() {
                                        self.context = (*body).to_string();
                                    }
                                }
                            });
                            ui.add(egui::TextEdit::multiline(&mut self.context).desired_rows(4).desired_width(f32::INFINITY)
                                .hint_text("System prompt / persistent context for every turn…"));
                            if !phys_params_on {
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
                            }
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
                                                crate::sandbox::Verdict::TestFailure => ("✗ tests failed", theme::RED(), "it compiled, but a test assertion failed — see the output below"),
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
        // Gated behind hide_bottom_bar: when hidden, this whole panel doesn't exist, so the spatial
        // view's CentralPanel gets the full remaining height instead of being cramped behind it.
        if !self.hide_bottom_bar {
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
                // live-transcription preview: while the mic is on, the forming utterance shows on the pearl
                // (dimmer, ahead of the committed text) so dictation has real-time feedback before it commits.
                let vpartial = self.voice.as_ref().map(|v| v.partial().to_string()).unwrap_or_default();
                let band = self.pearl_input.show(ui, &self.input, &vpartial, pearl_t, 132.0, theme::CYAN());
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
                // Composer command hint: when the line is a ':' command (and not a '::' escaped message),
                // show its usage + argument suggestions inline — the same palette the vim cmdline uses — so
                // commands typed straight into the message box are discoverable and self-documenting, and it's
                // visually obvious the line will RUN a command instead of being sent to the model.
                let ci = self.input.trim();
                if ci.starts_with(':') && !ci.starts_with("::") {
                    let raw = ci.trim_start_matches(':');
                    let mut parts = raw.splitn(2, char::is_whitespace);
                    let head = parts.next().unwrap_or("");
                    let arg_partial = parts.next().unwrap_or("");
                    if let Some(s) = crate::cmdpalette::spec(head) {
                        ui.label(RichText::new(format!("⌘ {}  ·  {}", s.usage, s.help)).color(theme::CYAN()).small());
                        let ctx = crate::cmdpalette::ArgCtx { models: Vec::new(), refs: vec!["-1".into(), "-2".into(), "-3".into()] };
                        let sugg = crate::cmdpalette::suggest_args(s, arg_partial, &ctx);
                        if !sugg.is_empty() {
                            let line = sugg.iter().take(4).map(|a| format!("{} — {}", a.value, a.hint)).collect::<Vec<_>>().join("    ");
                            ui.label(RichText::new(line).color(theme::FAINT()).small());
                        }
                    } else if !head.is_empty() {
                        let names: Vec<&str> = crate::cmdpalette::matches(head).iter().take(8).map(|s| s.name).collect();
                        ui.label(RichText::new(if names.is_empty() { "no match · :help to list commands".to_string() } else { format!("↹ {}", names.join("  ")) })
                            .color(theme::DIM()).small());
                    } else {
                        ui.label(RichText::new("command mode · :help to list · Enter runs it (not sent to the model)").color(theme::DIM()).small());
                    }
                }
                ui.add_space(theme::PAD_XS as f32);
                ui.horizontal(|ui| {
                    if theme::carved_button(ui, "📎 Attach").on_hover_text("Attach a text file into your message (bounded)").clicked() {
                        let start = self.save_dir.clone()
                            .or_else(|| std::env::var("HOME").ok().map(std::path::PathBuf::from))
                            .unwrap_or_else(|| std::path::PathBuf::from("/"));
                        self.attach_picker.open_file(&start, "Attach a file", &[]);
                    }
                    // Mic toggle: start/stop the live voice session. Dictates finalized speech into the composer
                    // (the routing seam in on_voice_utterance grows into graph commands next). Errors surface
                    // inline (no model/mic, or a build without the `voice` feature).
                    let miced = self.voice.is_some();
                    if theme::carved_button(ui, if miced { "🔴 Stop mic" } else { "🎙 Mic" })
                        .on_hover_text(
                            "Voice — speak; each utterance runs as a command or dictates.\n\
                             Nav: \"go to 12\" · \"focus on three\" · \"node seven\"\n\
                             Graph: \"branch here\" · \"prune\" · \"unprune\" · \"best\"\n\
                             View: \"overview\" · \"show the palace\" · \"back to chat\"\n\
                             Camera: \"start/stop tour\" · \"free cam\" / \"exit freecam\"\n\
                             Compose: \"send it\" · \"stop\" (cancel) · anything else dictates")
                        .clicked() {
                        if miced { self.voice = None; }
                        else {
                            match crate::voice_session::VoiceSession::start() {
                                Ok(s) => { self.voice = Some(s); self.voice_err = None; ui.ctx().request_repaint(); }
                                Err(e) => { self.voice_err = Some(format!("voice: {e}")); }
                            }
                        }
                    }
                    if let Some(v) = &self.voice {
                        let p = v.partial();
                        let shown = if p.is_empty() { "listening…".to_string() } else { p.chars().take(60).collect::<String>() };
                        ui.label(RichText::new(shown).color(theme::CYAN()).small().italics());
                        ui.ctx().request_repaint(); // keep transcription live while the mic is on
                    } else if let Some(e) = &self.voice_err {
                        ui.label(RichText::new(e).color(egui::Color32::from_rgb(0xe0, 0x6c, 0x6c)).small());
                    }
                    ui.label(RichText::new(if self.threading { "threaded" } else { "single-shot" }).color(theme::FAINT()).small());
                    ui.checkbox(&mut self.stepwise, RichText::new("stepwise").small())
                        .on_hover_text("checkpoint mode: the model works in numbered steps, pausing at ⏸ STEP k/n markers; the harness continues for you (warm-KV). Each pause is a fan-out injection point.");
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
                            // PHASE on the button: prefill (reading the prompt — the long silent gap before
                            // any token) reads as "thinking"; once tokens flow it's "drafting" the reply.
                            let (phase, pcol) = match self.stream.as_ref() {
                                Some(s) if s.tokens == 0 => ("✦ thinking", theme::CYAN()),
                                _ => ("✍ drafting", theme::AMBER()),
                            };
                            theme::spinner(ui, 16.0, pcol);
                            if theme::action_button(ui, "⏹ Stop", theme::RED(), true).clicked() { infer.cancel(); }
                            ui.label(RichText::new(phase).color(pcol).small())
                                .on_hover_text(if pcol == theme::CYAN() { "prefilling the prompt (no tokens yet)" } else { "decoding the reply" });
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
        }

        // ── center: the message log fills whatever height remains above the pinned bottom ──
        egui::CentralPanel::default().show_inside(ui, |ui| {
            if self.spatial_view {
                let want_send = self.spatial_ui(ui, Some(model));
                if want_send { self.send(infer, doc, ui); }
                return;
            } // aggressive: spatial graph replaces the linear log
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
                                                // ── file summary toggle (gitmind integration) ──
                                                // Blocks naming a file get a [✨ Summary | Raw] switch. Files inside the
                                                // loaded .gitmind session's /tmp workspace DEFAULT to summary — the
                                                // session's own files are the ones you want described, not re-scrolled.
                                                // Summary = the instant structural digest, upgraded in place by the
                                                // small-model summary once the ollama sidecar delivers it (cached by
                                                // content hash; see filesummary.rs).
                                                let fpath = crate::filesummary::code_block_path(lang);
                                                let in_session = fpath.as_deref().map(|p| {
                                                    self.gitmind_session.as_ref().is_some_and(|s| crate::filesummary::is_session_file(p, s.work_dir()))
                                                }).unwrap_or(false);
                                                let sum_id = egui::Id::new(("codesum", i, si));
                                                let mut show_summary = fpath.is_some()
                                                    && ui.ctx().data_mut(|d| *d.get_temp_mut_or(sum_id, in_session));
                                                egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(6),
                                                    inner_margin: egui::Margin::same(theme::PAD_SM), stroke: egui::Stroke::new(1.0, theme::LINE()), ..Default::default() }
                                                .show(ui, |ui| {
                                                    ui.horizontal(|ui| {
                                                        let l = lang.split_whitespace().next().unwrap_or("");
                                                        if !l.is_empty() { ui.label(RichText::new(l).color(theme::md_accent()).small().monospace()); }
                                                        if let Some(p) = &fpath {
                                                            let mut sw = show_summary;
                                                            if ui.selectable_label(sw, RichText::new("\u{2728} Summary").small()).on_hover_text("Describe this file instead of showing the code").clicked() { sw = true; }
                                                            if ui.selectable_label(!sw, RichText::new("Raw").small()).clicked() { sw = false; }
                                                            if sw != show_summary {
                                                                show_summary = sw;
                                                                ui.ctx().data_mut(|d| d.insert_temp(sum_id, sw));
                                                            }
                                                            if in_session { ui.label(RichText::new("session").color(theme::md_dim()).small()).on_hover_text(format!("{p} is in the gitmind /tmp workspace")); }
                                                        }
                                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                            if ui.small_button("Copy").on_hover_text("Copy this block").clicked() { ui.ctx().copy_text(body.clone()); }
                                                        });
                                                    });
                                                    if show_summary {
                                                        let p = fpath.as_deref().unwrap_or("");
                                                        let key = crate::filesummary::content_key(p, body);
                                                        crate::filesummary::request_model_summary(key, p, body);
                                                        match crate::filesummary::model_summary(key) {
                                                            Some(text) => { render_markdown(ui, &text, ""); }
                                                            None => {
                                                                render_markdown(ui, &crate::filesummary::structural_digest(p, body), "");
                                                                let note = if std::env::var("LOGSCOPE_OLLAMA").is_ok() && crate::filesummary::in_flight(key) {
                                                                    "\u{2728} model summary generating…"
                                                                } else {
                                                                    "structural digest (set LOGSCOPE_OLLAMA for model summaries)"
                                                                };
                                                                ui.label(RichText::new(note).color(theme::FAINT()).small().italics());
                                                                if crate::filesummary::in_flight(key) { ui.ctx().request_repaint_after(std::time::Duration::from_millis(500)); }
                                                            }
                                                        }
                                                    } else {
                                                        ui.label(RichText::new(format!("{body}{tail}")).monospace().color(theme::md_ink()));
                                                    }
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
        if do_send { self.send(infer, doc, ui); }
        // Drain one queued (branch) turn as soon as the model is free — serial, but non-blocking from
        // the user's side: they fired it mid-generation and it runs the moment the current reply ends.
        if self.stream.is_none() && self.agent_run.is_none() && self.agent_fan.is_none() {
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
        // Stepwise auto-continue: resume the paused reply on the user's behalf (warm-KV, one per pause).
        // If a cargo-test run is in flight for this pause, wait for its result and splice it in first.
        if self.step_arm_continue && self.stream.is_none() {
            if let Some(rx) = &self.step_shell {
                match rx.try_recv() {
                    Ok(msg) => { self.step_inject = Some(msg); self.step_shell = None; }
                    Err(std::sync::mpsc::TryRecvError::Empty) => { ui.ctx().request_repaint(); return; } // keep waiting
                    Err(_) => { self.step_shell = None; }
                }
            }
            self.step_arm_continue = false;
            if let Some(msg) = self.step_inject.take() {
                if let Some(m) = self.msgs.last_mut() {
                    m.cur_mut().push_str(&format!("\n\n{msg}\n"));
                }
            }
            self.continue_generation(infer, doc);
        }
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
    fn project_slab_rect_maps_a_facing_slab_around_screen_centre() {
        use crate::spatial::{OrbitCam, Vec3};
        let cam = OrbitCam::default();
        let (cx, cy, scale) = (640.0_f32, 400.0_f32, 600.0_f32);
        // a 2×2 slab AT the camera target, spanned by the world x/y axes → its centre projects to the screen
        // centre, so the bounding rect must straddle (cx,cy) and have positive area.
        let r = project_slab_rect(&cam, cam.target, Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0),
                                  2.0, 2.0, cx, cy, scale)
            .expect("a slab at the target is in front of the camera");
        assert!(r.width() > 0.0 && r.height() > 0.0, "positive-area rect: {r:?}");
        assert!(r.contains(egui::pos2(cx, cy)), "rect straddles screen centre: {r:?}");
        // a wider slab projects to a wider rect (monotonic in world size).
        let r2 = project_slab_rect(&cam, cam.target, Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0),
                                   4.0, 2.0, cx, cy, scale).unwrap();
        assert!(r2.width() > r.width(), "wider slab → wider rect ({} vs {})", r2.width(), r.width());
    }

    #[test]
    fn phys_dock_press_release_maps_to_the_camera_action() {
        use crate::physui::{ControlEvent, Pointer};
        let mut p = phys_dock();
        // a full press→release over Overview actuates it → the recenter action.
        p.on_pointer(Pointer::Down, Some(DOCK_OVERVIEW));
        let ev = p.on_pointer(Pointer::Up, Some(DOCK_OVERVIEW)).expect("overview fires");
        assert_eq!(dock_action(ev), Some(DockAction::Overview));
        // Tour is a toggle: first press → on, second → off, each mapping through.
        p.on_pointer(Pointer::Down, Some(DOCK_TOUR));
        let on = p.on_pointer(Pointer::Up, Some(DOCK_TOUR)).unwrap();
        assert_eq!(dock_action(on), Some(DockAction::Tour(true)));
        p.on_pointer(Pointer::Down, Some(DOCK_TOUR));
        let off = p.on_pointer(Pointer::Up, Some(DOCK_TOUR)).unwrap();
        assert_eq!(dock_action(off), Some(DockAction::Tour(false)));
        // Freecam toggles the same way.
        p.on_pointer(Pointer::Down, Some(DOCK_FREECAM));
        let fc = p.on_pointer(Pointer::Up, Some(DOCK_FREECAM)).unwrap();
        assert_eq!(dock_action(fc), Some(DockAction::Freecam(true)));
        // Prune is momentary → fires once per press→release.
        p.on_pointer(Pointer::Down, Some(DOCK_PRUNE));
        let pr = p.on_pointer(Pointer::Up, Some(DOCK_PRUNE)).expect("prune fires");
        assert_eq!(dock_action(pr), Some(DockAction::Prune));
        // Unprune (ported from the old 2D toolbar) is momentary too.
        p.on_pointer(Pointer::Down, Some(DOCK_UNPRUNE));
        let un = p.on_pointer(Pointer::Up, Some(DOCK_UNPRUNE)).expect("unprune fires");
        assert_eq!(dock_action(un), Some(DockAction::Unprune));
        // the dock registers exactly the five named controls.
        assert_eq!(DOCK_IDS.len(), 5);
        // external sync keeps the physical toggle consistent with app state set elsewhere.
        p.set_active(DOCK_TOUR, true);
        assert!(p.is_active(DOCK_TOUR), "set_active lights the physical Tour toggle");
        // events that aren't a dock control are ignored by the mapping.
        assert_eq!(dock_action(ControlEvent::Fired(9999)), None);
        assert_eq!(dock_action(ControlEvent::Adjusted(DOCK_TOUR, 0.5)), None);
    }

    #[test]
    fn wrap_label_wraps_splits_and_truncates() {
        // short text → one line, untouched
        assert_eq!(wrap_label("hello there", 20, 3), vec!["hello there"]);
        // wraps on whitespace within the column budget
        let w = wrap_label("the quick brown fox jumps", 10, 3);
        assert!(w.iter().all(|l| l.chars().count() <= 10), "each line within cols: {w:?}");
        assert_eq!(w[0], "the quick");
        // collapses newlines/extra spaces
        assert_eq!(wrap_label("a\n\n  b", 20, 2), vec!["a b"]);
        // hard-splits an over-long word
        let hw = wrap_label("supercalifragilistic", 6, 5);
        assert_eq!(hw[0], "superc");
        // truncation marks the last line with …
        let t = wrap_label("one two three four five six seven eight", 8, 2);
        assert_eq!(t.len(), 2);
        assert!(t[1].ends_with('…'), "truncated last line ends with …: {t:?}");
    }

    #[test]
    fn settings_dials_drag_within_range_and_bind_to_ctx() {
        use crate::physui::ControlEvent;
        let mut p = phys_dock();
        // temperature dial: 0..1.5, starts at the Ctx default; drag +0.5 of range → +0.75
        p.set_value(PARAM_TEMP, 0.0);
        match p.drag(PARAM_TEMP, 0.5) {
            Some(ControlEvent::Adjusted(id, v)) => { assert_eq!(id, PARAM_TEMP); assert!((v - 0.75).abs() < 1e-3, "0.0 +0.5·1.5 = 0.75, got {v}"); }
            other => panic!("expected Adjusted, got {other:?}"),
        }
        // clamps at the range ends
        p.drag(PARAM_TEMP, 5.0);
        assert!((p.value_of(PARAM_TEMP) - 1.5).abs() < 1e-3, "clamps at max 1.5");
        // ctx_get / ctx_set round-trip, with integer params rounded
        let mut c = Ctx::default();
        ctx_set(&mut c, PARAM_NCTX, 8000.4);
        assert_eq!(c.n_ctx, 8000, "ctx window written + rounded");
        assert!((ctx_get(&c, PARAM_NCTX) - 8000.0).abs() < 1e-3);
        ctx_set(&mut c, PARAM_TEMP, 0.9);
        assert!((c.temperature - 0.9).abs() < 1e-6, "temperature written");
        // every registered param dial reads back its range end after a big drag
        for &(id, _, _lo, hi, _) in &PARAM_DIALS {
            p.drag(id, 9.0);
            assert!((p.value_of(id) - hi).abs() < 1e-3, "dial {id} clamps at its max {hi}");
        }
    }

    #[test]
    fn mic_shape_is_a_coherent_per_scene_derivative() {
        use crate::theme::Material::*;
        // crystalline worlds → faceted gem (the pass-67 second axis), regardless of their stone material.
        for s in ["Fractal Spires", "Recursive Citadel", "Frozen Bifrost"] {
            assert_eq!(mic_shape(s, Stone), MicShape::Gem, "{s} is crystalline");
        }
        // foliage worlds → a leaf (signature object), even though their palette material is wood.
        for s in ["Forest", "Canopy", "Treehouse", "Hollow Jungle"] {
            assert_eq!(mic_shape(s, Wood), MicShape::Leaf, "{s} is foliage");
        }
        // watery / airy worlds → smooth pearl; cave/mist → stone.
        for s in ["Atlantis", "Cloudscape"] {
            assert_eq!(mic_shape(s, Stone), MicShape::Pearl, "{s} is watery/airy");
        }
        assert_eq!(mic_shape("Mistfall", Stone), MicShape::Stone, "Mistfall is cave/rune stone");
        // otherwise the shape follows the physical material.
        assert_eq!(mic_shape("Old Library", Wood), MicShape::Bead, "wood → turned bead");
        assert_eq!(mic_shape("Slate", Stone), MicShape::Stone, "stone → weathered bead");
        assert_eq!(mic_shape("Mansion Garden", Marble), MicShape::Stone, "marble → carved bead");
        assert_eq!(mic_shape("Reading Room", Paper), MicShape::Pearl, "paper → wafer/pearl");
        // and every registered palette resolves to some shape (no scene falls through to a panic/None).
        for p in crate::theme::PALETTES {
            let _ = mic_shape(p.name, p.material); // total over the real scene set
        }
    }

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
        // a summary, not a 1000-line dump: measure the LOG BLOCK itself (the delta the
        // attachment adds over the base persona), so this bound is independent of how big
        // the selected system persona happens to be. 1000 raw lines would be ~20 KB.
        let base = c.full_system(&Doc::from_text("app.log", ""));
        let block = fs.len() - base.len();
        assert!(block < 1500, "log block must stay bounded, got {block} chars");
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
    fn continue_fan_pins_to_one_line() {
        // same branched tree: leaves are 1 (linear line) and 3 (the fork off 0)
        let mut c = Chat::default();
        c.msgs.push(Msg::user("q1".into()));           // 0
        c.msgs.push(Msg::assistant());                 // 1  (line A tip)
        let mut u = Msg::user("q1-alt".into()); u.parent = Some(0); c.msgs.push(u); // 2
        let mut a = Msg::assistant(); a.parent = Some(2); c.msgs.push(a);           // 3 (line B tip)
        // leaf_under descends the fork to its live tip
        assert_eq!(c.leaf_under(2), Some(3), "the tip under anchor 2 is leaf 3");
        assert_eq!(c.leaf_under(0), Some(1), "anchor 0's linear-fallback tip is 1");
        // :continue anchored to line B (node 2) queues a continuation for leaf 3 ONLY — not line A's leaf 1
        c.fan = Some(Fan { kind: FanKind::Continue, rounds_left: 2, only: Some(2) });
        c.fan_step();
        assert_eq!(c.queue.len(), 1, "single-line continue expands exactly one leaf per round");
        assert_eq!(c.queue[0].1, Some(3), "…and it's the tip of the anchored line, not the other branch");
    }

    #[test]
    fn compose_input_routes_commands_vs_messages() {
        use crate::vimmode::Command;
        // a leading ':' is a command, not a chat turn — this is the whole point of composer routing
        assert!(matches!(compose_input(":fuzz 8"), Composed::Command(Command::Fuzz(8))));
        assert!(matches!(compose_input("  :ca 3  "), Composed::Command(Command::ContinueAll(3))));
        assert!(matches!(compose_input(":stop"), Composed::Command(Command::Stop)));
        // bare number = goto (vim-style)
        assert!(matches!(compose_input(":5"), Composed::Command(Command::Goto(5))));
        // an unknown command still routes as a command (send() surfaces it, never ships it to the model)
        assert!(matches!(compose_input(":definitelynotacommand"), Composed::Command(Command::Unknown(_))));
        // '::' escapes a literal message that begins with a colon
        match compose_input("::look at line :50") { Composed::Message(m) => assert_eq!(m, "look at line :50"), _ => panic!(":: should be a literal message") }
        // ordinary text is a chat turn, untouched
        match compose_input("how does the parser work?") { Composed::Message(m) => assert_eq!(m, "how does the parser work?"), _ => panic!("plain text is a message") }
        assert!(matches!(compose_input("   "), Composed::Empty));
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
    fn activity_status_summarizes_inflight_work() {
        let mut c = Chat::default();
        assert_eq!(c.activity_status(), None, "idle → nothing to show");
        c.queue.push_back(("x".into(), None));
        c.queue.push_back(("y".into(), None));
        assert_eq!(c.activity_status().as_deref(), Some("2 queued"));
        // a running depth fan reads alongside the queued count
        c.fan = Some(Fan { kind: FanKind::Continue, rounds_left: 3, only: None });
        let s = c.activity_status().unwrap();
        assert!(s.contains("2 queued") && s.contains("3 rounds left"), "combines queue + fan: {s}");
        // singular grammar for the last round
        c.queue.clear();
        c.fan = Some(Fan { kind: FanKind::Continue, rounds_left: 1, only: None });
        assert_eq!(c.activity_status().as_deref(), Some("1 round left"));
        // an exhausted fan (0 rounds) contributes nothing → idle
        c.fan = Some(Fan { kind: FanKind::Continue, rounds_left: 0, only: None });
        assert_eq!(c.activity_status(), None);
    }

    #[test]
    fn active_persona_label_names_the_system_persona() {
        // Chat::default() seeds the context with the long-horizon persona
        assert_eq!(Chat::default().active_persona_label(), "Long-horizon engineer");
        let mut c = Chat::default();
        c.context = BASE_SYSTEM.to_string();
        assert_eq!(c.active_persona_label(), "Logscope · quick log fix");
        let mut c2 = Chat::default();
        c2.context = "You are a haiku bot.".into();
        assert_eq!(c2.active_persona_label(), "custom · system");
    }

    #[test]
    fn persona_switch_round_trips_through_the_badge() {
        // the status-bar popup switches persona with persona_by_key(key); the badge then reflects it
        let mut c = Chat::default();
        for (key, title, _) in PERSONAS {
            c.context = persona_by_key(key).expect("known key resolves").to_string();
            assert_eq!(c.active_persona_label(), *title, "badge follows the picked persona: {key}");
        }
        assert!(persona_by_key("nope").is_none(), "unknown key → None");
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
    fn segments_longer_fence_wraps_inner_backticks() {
        // a 4-backtick block containing a full ``` run stays ONE code block (the old toggle split it)
        let segs = segment_message("````md doc\nuse ```rust\nfn x(){}\n``` inline\n````\nafter");
        assert_eq!(segs.len(), 2, "outer 4-fence should not be closed by the inner 3-backtick run");
        match &segs[0] { Seg::Code { body, .. } => assert!(body.contains("```rust") && body.contains("``` inline")), _ => panic!("expected code") }
        assert!(matches!(&segs[1], Seg::Prose(p) if p == "after"));
    }

    #[test]
    fn segments_info_string_fence_does_not_close() {
        // ```diff has an info string, so it can only OPEN — a bare ``` closes. The old toggle let ```diff
        // "close" a block and desynced everything after (the reported mangling).
        let segs = segment_message("```diff\n-a\n+b\n```\ntail prose");
        assert_eq!(segs.len(), 2);
        match &segs[0] { Seg::Code { lang, body } => { assert_eq!(lang, "diff"); assert_eq!(body, "-a\n+b"); }, _ => panic!("expected code") }
        assert!(matches!(&segs[1], Seg::Prose(p) if p == "tail prose"));
    }

    #[test]
    fn attachment_fence_survives_embedded_backticks() {
        // a file whose content contains ``` must be wrapped in a LONGER fence so it can't close early
        let block = fenced_attachment("readme.md", "md", b"# Title\n```rust\nfn x(){}\n```\ndone");
        // re-parsing the block should yield exactly one code segment holding the whole file
        let segs = segment_message(block.trim());
        assert_eq!(segs.len(), 1, "embedded ``` must not split the attachment");
        match &segs[0] { Seg::Code { body, .. } => assert!(body.contains("```rust") && body.contains("done")), _ => panic!("expected one code block") }
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

    /// Regression guard: after a completed turn (live tip on the assistant), sending a new message must
    /// put THAT message as the final user turn in the prompt — not drop it for the second-to-last.
    #[test]
    fn last_user_message_reaches_prompt() {
        let mut c = Chat::default();
        c.backend = BackendKind::Wgpu;
        c.template = TemplateKind::Llama3;
        c.attach_log = false;
        c.msgs.push(Msg::user("FIRST_QUESTION".into()));
        let mut a0 = Msg::assistant(); a0.variants[0] = "FIRST_ANSWER".into(); a0.parent = Some(0);
        c.msgs.push(a0);
        c.active_leaf = Some(1);
        // replicate start_generation's prompt-build prefix for the second send
        let parent = c.branch_from.take().or(c.active_leaf).or(c.msgs.len().checked_sub(1));
        c.msgs.push(Msg::user("SECOND_QUESTION".into()));
        let uid = c.msgs.len() - 1;
        c.msgs[uid].parent = parent;
        c.active_leaf = Some(uid);
        let p = c.build_prompt(&Doc::from_text("", ""));
        let last_user = p.rfind("user<|end_header_id|>").map(|i| &p[i..]).unwrap_or("");
        assert!(last_user.contains("SECOND_QUESTION"), "last USER turn is not the newest message");
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

#[cfg(test)]
mod long_horizon_prompt_tests {
    #[test]
    fn long_horizon_prompt_loaded_with_core_disciplines() {
        let p = super::LONG_HORIZON;
        assert!(p.len() > 1500, "the long-horizon prompt is substantial, got {}", p.len());
        // the hard-won disciplines must be present (guards the include_str! wiring + content)
        assert!(p.contains("Search before you build") || p.to_lowercase().contains("search the tree"),
            "must encode search-first / no-duplication");
        assert!(p.contains("Priorities"), "must state the priority ladder");
        assert!(p.to_lowercase().contains("linux"), "must be Linux-first");
        assert!(p.to_lowercase().contains("musl"), "must respect musl");
        // it's the default persona
        assert_eq!(super::PERSONAS[0].0, "longhorizon");
        assert_eq!(super::PERSONAS[0].2, p);
    }
}
