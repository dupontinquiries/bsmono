//! roll — the piano-roll surface for `studio-live`'s held notes.
//!
//! The grid answers "which steps fire"; the roll answers "what is held, how
//! long, and together with what". Same contract as `grid`: **nothing here
//! owns the pattern** — every interaction returns a [`RollEdit`] for the host
//! to apply, because an edit with no representation is invisible to a
//! collaborator (the grid module's own words, still true one panel over).
//!
//! ## Keys are offsets, not absolute pitches
//!
//! A row is a semitone offset from the channel's own pitch — the same
//! convention `SynthKind::Sub`'s `note` and `StepMod::tune` use — so the
//! gutter labels octaves (`+12`, `0`, `-12`) rather than pretending to know
//! that a sampler chop is a C. Row 0 is the channel as it already sounds.
//!
//! ## One gesture, one intention
//!
//! Press an empty cell and drag right: one note, sized as you go — the add
//! and every resize are separate edits on the wire, but the LAST resize is
//! the length that sticks, and a peer watching sees the note grow exactly as
//! the finger meant it. Click an empty cell: a note of the LAST length you
//! drew (FL's rule — a bass line of eighths is eighths without re-sizing
//! each one). Grab a note's body and drag: it moves, in time and pitch, as a
//! ghost until you let go — ONE edit on drop, so dragging across another note
//! never deletes it on the way. Grab its right edge: resize.
//!
//! Click a note: select it (outlined; the arrow keys then move and transpose
//! it). Right-click — or a long press, on touch — deletes. Double-click
//! toggles slide. Until c20 a plain click deleted, which is the gesture a
//! mouse makes most often and by accident; and there was no move at all.
//!
//! Hovering names what is under the pointer ([`readout`]) and sets the
//! cursor: resize at an edge, grab on a body.

use studio_live::{Note, Pattern};

/// Column width — [`crate::grid::CELL`], so a roll column sits exactly under
/// the grid column it shares a step with.
pub const CELL: f32 = crate::grid::CELL;
/// Row height. Shorter than a grid cell: a useful roll shows two octaves
/// plus, and 25 rows of 22px would not fit a phone viewport.
pub const KEY_H: f32 = 14.0;
/// The note-name gutter: [`crate::grid::LABEL_W`], the SAME as the step
/// grid's channel-name gutter. It was 44 until c20, against the grid's 96, so
/// every step column jumped 52 px sideways when you switched between the two
/// views of one pattern -- the column under your eye was a different step.
pub const LABEL_W: f32 = crate::grid::LABEL_W;
/// How close to a note's right edge counts as grabbing the edge.
pub const EDGE_W: f32 = 6.0;

/// What the roll asks the host to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RollEdit {
    Add { channel: usize, note: Note },
    Remove { channel: usize, start: u32, key: i8 },
    Resize { channel: usize, start: u32, key: i8, len: u32 },
    ToggleSlide { channel: usize, start: u32, key: i8 },
    /// Move a note, keeping its length, velocity and slide
    /// (`Pattern::move_note`).
    Move { channel: usize, start: u32, key: i8, to_start: u32, to_key: i8 },
    /// Velocity for every note starting at `start` — a chord is one
    /// gesture and carries one dynamic (see `Pattern::set_start_vel`).
    SetVel { channel: usize, start: u32, vel: f32 },
}

/// Velocity-lane height.
pub const VEL_H: f32 = 40.0;

/// y within the velocity lane → 0..=1, top loud, bottom silent.
pub fn vel_from_y(y: f32, top: f32) -> f32 {
    (1.0 - (y - top) / VEL_H).clamp(0.0, 1.0)
}

/// The velocity lane under the roll: one draggable bar per start column
/// that has notes, mirroring `lanes`' bar grammar so the two read as one
/// family. Columns without notes are inert — a velocity with nothing to
/// carry it would be an edit to nowhere.
pub fn vel_lane(ui: &mut egui::Ui, pattern: &Pattern, channel: usize,
                c: &uikit_egui::PaletteColors) -> Vec<RollEdit> {
    let mut edits = Vec::new();
    if pattern.steps == 0 { return edits; }
    let width = LABEL_W + pattern.steps as f32 * CELL;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, VEL_H),
                                              egui::Sense::click_and_drag());
    let painter = ui.painter();
    painter.text(egui::pos2(rect.left() + 4.0, rect.center().y),
                 egui::Align2::LEFT_CENTER, "vel",
                 egui::FontId::proportional(10.0), c.text_weak);
    for s in 0..pattern.steps {
        let x = rect.left() + LABEL_W + s as f32 * CELL;
        let col = egui::Rect::from_min_size(egui::pos2(x + 1.0, rect.top()),
                                            egui::vec2(CELL - 2.0, VEL_H));
        painter.rect_filled(col, 0.0,
            if crate::grid::is_beat(s) { c.bg_raised } else { c.bg_base });
        if let Some(v) = pattern.start_vel(channel, s) {
            let bh = (v.clamp(0.0, 1.0) * VEL_H).max(2.0);
            let bar = egui::Rect::from_min_size(
                egui::pos2(x + 3.0, rect.bottom() - bh),
                egui::vec2(CELL - 6.0, bh));
            painter.rect_filled(bar, 1.0, c.accent);
        }
    }
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() {
            let rel_x = pos.x - rect.left();
            if rel_x >= LABEL_W {
                let s = ((rel_x - LABEL_W) / CELL) as u32;
                if s < pattern.steps && pattern.start_vel(channel, s).is_some() {
                    edits.push(RollEdit::SetVel {
                        channel, start: s, vel: vel_from_y(pos.y, rect.top()) });
                }
            }
        }
    }
    edits
}

/// Height of [`step_echo`].
pub const ECHO_H: f32 = 8.0;

/// A thin strip above the roll marking the steps this channel's STEP GRID
/// fires -- the Steps view seen from here, on the same columns (the gutters
/// match), so a note drawn over a kick is drawn knowing the kick is there.
/// Draw-only: the grid is where steps are edited.
pub fn step_echo(ui: &mut egui::Ui, pattern: &Pattern, channel: usize, c: &uikit_egui::PaletteColors) {
    if pattern.steps == 0 { return; }
    let width = LABEL_W + pattern.steps as f32 * CELL;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, ECHO_H), egui::Sense::hover());
    let painter = ui.painter();
    painter.text(egui::pos2(rect.left() + 6.0, rect.center().y), egui::Align2::LEFT_CENTER, "steps",
                 egui::FontId::proportional(8.0), c.text_weak);
    for s in 0..pattern.steps {
        if pattern.is_set(channel, s) {
            let x = rect.left() + LABEL_W + s as f32 * CELL;
            painter.rect_filled(egui::Rect::from_min_size(egui::pos2(x + 3.0, rect.top() + 2.0),
                                egui::vec2(CELL - 6.0, ECHO_H - 3.0)), 1.0, c.accent.gamma_multiply(0.6));
        }
    }
}

/// A resize in progress — either a fresh note being sized as it is dragged
/// out, or an existing note grabbed by its right edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteDrag {
    pub start: u32,
    pub key: i8,
    /// The last length emitted, so a held pointer does not re-emit sixty
    /// identical resizes a second to every collaborator — `Paint::last`'s
    /// reasoning, one panel over.
    pub last_len: u32,
}

/// A pointer gesture in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    /// A new note being drawn out; its resizes go on the wire as it grows.
    Draw(NoteDrag),
    /// An existing note's right edge.
    Resize(NoteDrag),
    /// A note's body, dragged. `grab` is how many steps into the note it was
    /// grabbed, so the note does not jump to put its START under the pointer;
    /// `to` is where it would land now -- drawn as a ghost, sent on drop.
    Move { start: u32, key: i8, grab: u32, to: (u32, i8) },
}

/// Everything the roll remembers between frames. The host keeps one per
/// roll; nothing in it is on the wire -- a selection is where YOUR eyes are.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RollState {
    pub gesture: Option<Gesture>,
    /// The selected note, `(start, key)`.
    pub selected: Option<(u32, i8)>,
    /// The length a click places: the last one drawn or resized (0 = 1).
    pub last_len: u32,
    /// What the pointer is over, for the readout and the highlight.
    pub hover: Option<RollHit>,
    /// The note the last click ADDED -- a quick second click lands on it and
    /// is a double-click, which must not toggle slide on a note that exists
    /// only because of the first click.
    pub just_added: Option<(u32, i8)>,
}

/// Where a moved note lands: `grab` steps behind the pointer's column,
/// clamped so it starts inside the pattern; pitch follows the pointer's row.
pub fn move_target(grab: u32, step_under: u32, key_under: i8, steps: u32) -> (u32, i8) {
    (step_under.saturating_sub(grab).min(steps.saturating_sub(1)), key_under)
}

/// The [`RollKey`] an arrow / Delete key press means, reading SHIFT from the
/// key event itself. `None` for any other key, or with ctrl / alt / command
/// held (those belong to someone else).
///
/// From the event, not the frame's modifier state: a synthetic shift+Right
/// (and a fast real one) arrives with shift and the arrow in one frame, and
/// the frame state read "no shift" -- shift+Right MOVED the note instead of
/// lengthening it (seen in the running app, c20).
pub fn roll_key(key: egui::Key, m: egui::Modifiers) -> Option<RollKey> {
    if m.ctrl || m.alt || m.command || m.mac_cmd { return None; }
    Some(match (key, m.shift) {
        (egui::Key::ArrowUp, false) => RollKey::Up,
        (egui::Key::ArrowUp, true) => RollKey::OctaveUp,
        (egui::Key::ArrowDown, false) => RollKey::Down,
        (egui::Key::ArrowDown, true) => RollKey::OctaveDown,
        (egui::Key::ArrowLeft, false) => RollKey::Left,
        (egui::Key::ArrowLeft, true) => RollKey::Shorter,
        (egui::Key::ArrowRight, false) => RollKey::Right,
        (egui::Key::ArrowRight, true) => RollKey::Longer,
        (egui::Key::Delete | egui::Key::Backspace, _) => RollKey::Delete,
        _ => return None,
    })
}

/// A key pressed with a note selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollKey { Up, Down, OctaveUp, OctaveDown, Left, Right, Shorter, Longer, Delete }

/// What a key does to the selected note `n`: the edit and the selection
/// after it, or `None` when it would leave the pattern or the key range (a
/// transpose off the top of the roll is refused, not wrapped).
pub fn key_edit(channel: usize, n: Note, k: RollKey, steps: u32) -> Option<(RollEdit, Option<(u32, i8)>)> {
    let mv = |to_start: i64, to_key: i32| -> Option<(RollEdit, Option<(u32, i8)>)> {
        if to_start < 0 || to_start >= steps as i64 || to_key < i8::MIN as i32 || to_key > i8::MAX as i32 { return None; }
        let (ts, tk) = (to_start as u32, to_key as i8);
        Some((RollEdit::Move { channel, start: n.start, key: n.key, to_start: ts, to_key: tk }, Some((ts, tk))))
    };
    let (s, k8) = (n.start as i64, n.key as i32);
    match k {
        RollKey::Up => mv(s, k8 + 1),
        RollKey::Down => mv(s, k8 - 1),
        RollKey::OctaveUp => mv(s, k8 + 12),
        RollKey::OctaveDown => mv(s, k8 - 12),
        RollKey::Left => mv(s - 1, k8),
        RollKey::Right => mv(s + 1, k8),
        RollKey::Shorter => (n.len > 1).then(|| (RollEdit::Resize { channel, start: n.start, key: n.key, len: n.len - 1 },
                                                  Some((n.start, n.key)))),
        RollKey::Longer => (n.start + n.len < steps).then(|| (RollEdit::Resize { channel, start: n.start, key: n.key, len: n.len + 1 },
                                                              Some((n.start, n.key)))),
        RollKey::Delete => Some((RollEdit::Remove { channel, start: n.start, key: n.key }, None)),
    }
}

/// One line naming what the pointer is over -- "E4 · step 5" on an empty
/// cell, the note's span, velocity and slide on a note -- or the selected
/// note when the pointer is elsewhere. Steps are counted from 1, as the grid
/// counts beats.
pub fn readout(state: &RollState, pattern: &Pattern, channel: usize) -> Option<String> {
    let note = |start: u32, key: i8| pattern.roll(channel).iter().find(|n| n.start == start && n.key == key).copied();
    let describe = |n: Note| format!("{} · steps {}–{} ({}) · vel {:.0}%{}", studio_core::note_label(n.key),
        n.start + 1, n.start + n.len.max(1), n.len.max(1), n.vel * 100.0, if n.slide { " · slide" } else { "" });
    if let Some(Gesture::Move { to, .. }) = state.gesture {
        return Some(format!("→ {} · step {}", studio_core::note_label(to.1), to.0 + 1));
    }
    match state.hover {
        Some(RollHit::Empty { key, step }) =>
            Some(format!("{} · step {} · click: a note of {}", studio_core::note_label(key), step + 1, state.last_len.max(1))),
        Some(RollHit::Body { start, key }) | Some(RollHit::Edge { start, key }) => note(start, key).map(describe),
        None => state.selected.and_then(|(s, k)| note(s, k)).map(|n| format!("selected: {}", describe(n))),
    }
}

/// What a pointer position lands on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RollHit {
    /// An empty cell: (key, step).
    Empty { key: i8, step: u32 },
    /// A note's body.
    Body { start: u32, key: i8 },
    /// A note's right edge — the resize grip.
    Edge { start: u32, key: i8 },
}

/// The row a key draws on, counting down from `top`, or `None` when the key
/// is outside the drawn range.
pub fn row_of(key: i8, top: i8, rows: usize) -> Option<usize> {
    let r = top as i32 - key as i32;
    (r >= 0 && (r as usize) < rows).then_some(r as usize)
}

/// The key a row stands for. Inverse of [`row_of`] wherever both are defined.
pub fn key_of(row: usize, top: i8) -> i8 {
    (top as i32 - row as i32).clamp(i8::MIN as i32, i8::MAX as i32) as i8
}

/// Whether a key offset lands on a black key, RELATIVE TO THE CHANNEL'S OWN
/// PITCH — the shading is a ruler for counting semitones, not a claim about
/// concert pitch. `rem_euclid` so -1 (a semitone down) shades like +11.
pub fn is_black(key: i8) -> bool {
    matches!((key as i32).rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

/// The length a resize drag means when the pointer is over `step_under`:
/// through that column inclusive, and never shorter than one step — dragging
/// left of the start is asking for the shortest note, not a negative one.
pub fn len_to(start: u32, step_under: u32) -> u32 {
    step_under.saturating_sub(start) + 1
}

/// What `rel` (a position relative to the widget's top-left) lands on, or
/// `None` in the gutter or outside the pattern.
pub fn hit(rel: egui::Vec2, pattern: &Pattern, channel: usize, top: i8, rows: usize)
           -> Option<RollHit> {
    if rel.x < LABEL_W || rel.y < 0.0 { return None; }
    let step = ((rel.x - LABEL_W) / CELL) as u32;
    let row = (rel.y / KEY_H) as usize;
    if step >= pattern.steps || row >= rows { return None; }
    let key = key_of(row, top);
    match pattern.note_at(channel, key, step) {
        None => Some(RollHit::Empty { key, step }),
        Some(n) => {
            let right = LABEL_W + (n.start + n.len.max(1)) as f32 * CELL;
            if right - rel.x <= EDGE_W {
                Some(RollHit::Edge { start: n.start, key })
            } else {
                Some(RollHit::Body { start: n.start, key })
            }
        }
    }
}

/// Draw the roll for one channel. Returns edits for the host to apply.
///
/// `top`/`rows` pick the visible key range (the host scrolls by moving
/// `top`); `playhead` is the loop column the transport reports, drawn the
/// same way the grid draws it — an outline, never a fill.
/// How the roll treats the project's key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyView {
    /// Every row drawn the same — the honest default when no key is set.
    #[default]
    Off,
    /// Out-of-key rows dimmed and their labels faded: guidance you can
    /// see past. Notes there still PLAY and still edit — a scale is
    /// advice, and a tool that silently refused the blue note would be
    /// wrong about music.
    Dim,
}

#[allow(clippy::too_many_arguments)]
pub fn roll(ui: &mut egui::Ui, pattern: &Pattern, channel: usize,
            top: i8, rows: usize, state: &mut RollState,
            playhead: Option<u32>, song_key: Option<studio_core::Key>, view: KeyView,
            c: &uikit_egui::PaletteColors) -> Vec<RollEdit> {
    let mut edits = Vec::new();
    if pattern.steps == 0 || rows == 0 {
        ui.colored_label(c.text_weak, "No pattern loaded.");
        return edits;
    }

    let width = LABEL_W + pattern.steps as f32 * CELL;
    let height = rows as f32 * KEY_H;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height),
                                              egui::Sense::click_and_drag());
    let painter = ui.painter();

    // ── rows and columns ────────────────────────────────────────────────
    for row in 0..rows {
        let key = key_of(row, top);
        let y = rect.top() + row as f32 * KEY_H;
        let out_of_key = match (song_key, view) {
            (Some(k), KeyView::Dim) => !k.contains(key),
            _ => false,
        };
        for s in 0..pattern.steps {
            let x = rect.left() + LABEL_W + s as f32 * CELL;
            let cr = egui::Rect::from_min_size(egui::pos2(x + 0.5, y + 0.5),
                                               egui::vec2(CELL - 1.0, KEY_H - 1.0));
            // Black-key rows sit darker; beat columns lift lighter rows the
            // way the grid does — both are rulers, and without them a roll
            // is an undifferentiated field you lose your place in.
            let mut fill = if is_black(key) { c.bg_base }
                       else if crate::grid::is_beat(s) { c.bg_raised }
                       else { c.bg_base.gamma_multiply(1.4) };
            // Out of key: dimmed, never disabled.
            if out_of_key { fill = fill.gamma_multiply(0.55); }
            painter.rect_filled(cr, 1.0, fill);
        }
        // The octave line at each ±12 boundary, and the channel's own pitch
        // (key 0) slightly heavier still — home has to be findable at a
        // glance or every melody starts with counting rows.
        if (key as i32).rem_euclid(12) == 0 {
            let w = if key == 0 { 1.5 } else { 1.0 };
            painter.line_segment([egui::pos2(rect.left() + LABEL_W, y),
                                  egui::pos2(rect.right(), y)],
                                 egui::Stroke::new(w, c.text_weak));
        }
        // EVERY row names itself — A5, C#4 — instead of only the octave
        // boundaries carrying a signed offset from the channel's pitch.
        // "+7" is arithmetic; "E4" is music, and the label is exact
        // because studio_core::note_label reads the same reference pitch
        // the synth does.
        let name = studio_core::note_label(key);
        let strong = key == 0 || name.starts_with('C');
        let col = if out_of_key { c.text_weak.gamma_multiply(0.6) }
                  else if strong { c.text } else { c.text_weak };
        painter.text(egui::pos2(rect.left() + 4.0, y + KEY_H * 0.5),
                     egui::Align2::LEFT_CENTER, name,
                     egui::FontId::proportional(if strong { 10.0 } else { 9.0 }), col);
    }

    // The hovered row and column: a faint band, so the pointer's pitch and
    // step read against the gutter and the ruler without counting.
    if let Some(h) = state.hover {
        let (key, step) = match h { RollHit::Empty { key, step } => (key, step),
                                    RollHit::Body { start, key } | RollHit::Edge { start, key } => (key, start) };
        if let Some(row) = row_of(key, top, rows) {
            let y = rect.top() + row as f32 * KEY_H;
            painter.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.left(), y), egui::vec2(width, KEY_H)),
                                0.0, c.accent.gamma_multiply(0.10));
        }
        let x = rect.left() + LABEL_W + step as f32 * CELL;
        painter.rect_filled(egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(CELL, height)),
                            0.0, c.accent.gamma_multiply(0.06));
    }

    // ── notes ───────────────────────────────────────────────────────────
    let note_rect = |start: u32, key: i8, len: u32| -> Option<egui::Rect> {
        let row = row_of(key, top, rows)?;
        let y = rect.top() + row as f32 * KEY_H;
        let x = rect.left() + LABEL_W + start as f32 * CELL;
        let w = (len.max(1) as f32 * CELL - 2.0).max(4.0);
        Some(egui::Rect::from_min_size(egui::pos2(x + 1.0, y + 1.5), egui::vec2(w, KEY_H - 3.0)))
    };
    let moving = match state.gesture { Some(Gesture::Move { start, key, .. }) => Some((start, key)), _ => None };
    for n in pattern.roll(channel) {
        if n.start >= pattern.steps { continue; } // hidden by a shortened pattern
        let Some(nr) = note_rect(n.start, n.key, n.len) else { continue };
        // Velocity as brightness: a soft note reads soft. Floor at 0.35 so
        // a near-silent note is still findable to delete. A note being
        // moved dims where it was; its ghost shows where it will be.
        let mut col = c.accent.gamma_multiply(0.35 + 0.65 * n.vel.clamp(0.0, 1.0));
        if moving == Some((n.start, n.key)) { col = col.gamma_multiply(0.35); }
        painter.rect_filled(nr, 2.0, col);
        if n.slide {
            // The slide marker: a small notch on the note's leading edge,
            // where the glide actually happens.
            painter.line_segment([nr.left_top(), egui::pos2(nr.left() + 4.0, nr.center().y)],
                                 egui::Stroke::new(1.5, c.text));
            painter.line_segment([egui::pos2(nr.left() + 4.0, nr.center().y), nr.left_bottom()],
                                 egui::Stroke::new(1.5, c.text));
        }
        if state.selected == Some((n.start, n.key)) {
            painter.rect_stroke(nr.expand(1.0), 2.0, egui::Stroke::new(1.5, c.text), egui::StrokeKind::Outside);
        }
    }
    if let Some(Gesture::Move { start, key, to, .. }) = state.gesture {
        let len = pattern.note_at(channel, key, start).map(|n| n.len).unwrap_or(1);
        if let Some(g) = note_rect(to.0, to.1, len) {
            painter.rect_stroke(g, 2.0, egui::Stroke::new(1.5, c.accent), egui::StrokeKind::Inside);
        }
    }

    // The playhead: a column outline, not a fill — filling would hide the
    // notes under it, which are the thing you are watching for.
    if let Some(s) = playhead {
        if s < pattern.steps {
            let x = rect.left() + LABEL_W + s as f32 * CELL;
            let pr = egui::Rect::from_min_size(egui::pos2(x, rect.top()),
                                               egui::vec2(CELL, height));
            painter.rect_stroke(pr, 2.0, egui::Stroke::new(1.5_f32, c.warning),
                                egui::StrokeKind::Outside);
        }
    }

    // ── interaction ─────────────────────────────────────────────────────
    state.hover = resp.hover_pos().and_then(|p| hit(p - rect.min, pattern, channel, top, rows));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(match (state.gesture, state.hover) {
            (Some(Gesture::Move { .. }), _) => egui::CursorIcon::Grabbing,
            (Some(_), _) | (None, Some(RollHit::Edge { .. })) => egui::CursorIcon::ResizeHorizontal,
            (None, Some(RollHit::Body { .. })) => egui::CursorIcon::Grab,
            (None, Some(RollHit::Empty { .. })) => egui::CursorIcon::Crosshair,
            (None, None) => egui::CursorIcon::Default,
        });
    }
    let steps = pattern.steps;
    if let Some(pos) = resp.interact_pointer_pos() {
        let rel = pos - rect.min;
        let here = hit(rel, pattern, channel, top, rows);
        let step_under = if rel.x >= LABEL_W { Some((((rel.x - LABEL_W) / CELL) as u32).min(steps.saturating_sub(1))) } else { None };
        let key_under = key_of(((rel.y.max(0.0)) / KEY_H) as usize, top);
        if resp.drag_started() {
            // Judge the grab where the PRESS was, not where the pointer is
            // now: egui only calls it a drag once the pointer has moved past
            // its threshold, which can be a cell away. Until c20 this used the
            // current position, so a quick grab of a note's body read as a
            // press on the empty cell beside it -- and drew a new note there.
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
            let grabbed = hit(origin - rect.min, pattern, channel, top, rows);
            let origin_step = (((origin.x - rect.min.x - LABEL_W) / CELL).max(0.0) as u32).min(steps.saturating_sub(1));
            match grabbed {
                Some(RollHit::Empty { key, step }) => {
                    // Draw the note out in one gesture: add it at a step,
                    // and let the rest of the drag be its resize.
                    edits.push(RollEdit::Add { channel, note: Note {
                        key, start: step, len: 1, vel: 1.0, slide: false, nudge: 0.0 } });
                    state.gesture = Some(Gesture::Draw(NoteDrag { start: step, key, last_len: 1 }));
                    state.selected = Some((step, key));
                }
                Some(RollHit::Edge { start, key }) => {
                    let len = pattern.note_at(channel, key, start).map(|n| n.len).unwrap_or(1);
                    state.gesture = Some(Gesture::Resize(NoteDrag { start, key, last_len: len }));
                    state.selected = Some((start, key));
                }
                Some(RollHit::Body { start, key }) => {
                    let grab = origin_step.saturating_sub(start);
                    state.gesture = Some(Gesture::Move { start, key, grab, to: (start, key) });
                    state.selected = Some((start, key));
                }
                None => {}
            }
        }
        match &mut state.gesture {
            Some(Gesture::Draw(d)) | Some(Gesture::Resize(d)) => {
                if let Some(su) = step_under {
                    let len = len_to(d.start, su);
                    if len != d.last_len {
                        d.last_len = len;
                        edits.push(RollEdit::Resize { channel, start: d.start, key: d.key, len });
                    }
                }
            }
            Some(Gesture::Move { grab, to, .. }) => {
                if let Some(su) = step_under {
                    let key = key_under.clamp(key_of(rows.saturating_sub(1), top), top);
                    *to = move_target(*grab, su, key, steps);
                }
            }
            None => {}
        }
        if resp.clicked() && state.gesture.is_none() {
            match here {
                Some(RollHit::Empty { key, step }) => {
                    let len = state.last_len.max(1).min(steps - step);
                    edits.push(RollEdit::Add { channel, note: Note { key, start: step, len, vel: 1.0, slide: false, nudge: 0.0 } });
                    state.selected = Some((step, key));
                    state.just_added = Some((step, key));
                }
                Some(RollHit::Body { start, key }) | Some(RollHit::Edge { start, key }) => {
                    if resp.double_clicked() && state.just_added != Some((start, key)) {
                        edits.push(RollEdit::ToggleSlide { channel, start, key });
                    }
                    if !resp.double_clicked() { state.just_added = None; }
                    state.selected = Some((start, key));
                }
                None => { state.selected = None; }
            }
        }
        if resp.secondary_clicked() || resp.long_touched() {
            if let Some(RollHit::Body { start, key }) | Some(RollHit::Edge { start, key }) = here {
                edits.push(RollEdit::Remove { channel, start, key });
                if state.selected == Some((start, key)) { state.selected = None; }
                state.gesture = None;
            }
        }
    }
    if resp.drag_stopped() || (!resp.dragged() && !resp.is_pointer_button_down_on()) {
        match state.gesture.take() {
            Some(Gesture::Move { start, key, to, .. }) if to != (start, key) => {
                edits.push(RollEdit::Move { channel, start, key, to_start: to.0, to_key: to.1 });
                state.selected = Some(to);
            }
            Some(Gesture::Draw(d)) | Some(Gesture::Resize(d)) => state.last_len = d.last_len,
            _ => {}
        }
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern_with_note() -> Pattern {
        let mut p = Pattern::new("A", 16, 2);
        p.add_note(0, Note { key: 3, start: 4, len: 3, vel: 1.0, slide: false, nudge: 0.0 });
        p
    }

    #[test]
    fn rows_and_keys_are_inverses_across_the_drawn_range() {
        // Including negative keys — the roll reaches below the channel's own
        // pitch, and an off-by-one here draws every note a semitone wrong.
        let (top, rows) = (12i8, 25usize);
        for row in 0..rows {
            let key = key_of(row, top);
            assert_eq!(row_of(key, top, rows), Some(row), "key {key}");
        }
        assert_eq!(key_of(0, top), 12);
        assert_eq!(key_of(24, top), -12);
        assert_eq!(row_of(13, top, rows), None, "above the range");
        assert_eq!(row_of(-13, top, rows), None, "below the range");
    }

    #[test]
    fn black_key_shading_wraps_negative_offsets() {
        // -1 must shade like +11 — a semitone BELOW an octave line is the
        // white key at the top of the previous octave, not the black key a
        // naive `%` (which returns -1) would land on.
        assert!(is_black(1));
        assert!(!is_black(-1), "-1 wraps to +11, a white key");
        assert!(!is_black(0));
        assert!(!is_black(-12));
        assert!(is_black(-6));
    }

    #[test]
    fn a_hit_knows_empty_from_body_from_edge() {
        let p = pattern_with_note();
        let (top, rows) = (12i8, 25usize);
        let row = row_of(3, top, rows).unwrap() as f32;
        let mid_y = row * KEY_H + KEY_H * 0.5;
        // Middle of the note's second step: body.
        let body_x = LABEL_W + 5.5 * CELL;
        assert_eq!(hit(egui::vec2(body_x, mid_y), &p, 0, top, rows),
                   Some(RollHit::Body { start: 4, key: 3 }));
        // Just inside the note's right edge: the resize grip.
        let edge_x = LABEL_W + 7.0 * CELL - 2.0;
        assert_eq!(hit(egui::vec2(edge_x, mid_y), &p, 0, top, rows),
                   Some(RollHit::Edge { start: 4, key: 3 }));
        // One row down, same x: empty — hit testing is row-aware, the same
        // property the timeline pins for clips.
        assert_eq!(hit(egui::vec2(body_x, mid_y + KEY_H), &p, 0, top, rows),
                   Some(RollHit::Empty { key: 2, step: 5 }));
        // The gutter and the space past the pattern are nothing.
        assert_eq!(hit(egui::vec2(LABEL_W - 2.0, mid_y), &p, 0, top, rows), None);
        assert_eq!(hit(egui::vec2(LABEL_W + 20.0 * CELL, mid_y), &p, 0, top, rows), None);
        // Another channel does not see this channel's note.
        assert_eq!(hit(egui::vec2(body_x, mid_y), &p, 1, top, rows),
                   Some(RollHit::Empty { key: 3, step: 5 }));
    }

    #[test]
    fn the_velocity_lane_clamps_and_points_the_right_way_up() {
        let top = 50.0;
        assert_eq!(vel_from_y(top, top), 1.0, "the lane's top is loud");
        assert_eq!(vel_from_y(top + VEL_H, top), 0.0, "its bottom is silent");
        assert!((vel_from_y(top + VEL_H * 0.5, top) - 0.5).abs() < 1e-6);
        assert_eq!(vel_from_y(top - 20.0, top), 1.0, "above clamps");
        assert_eq!(vel_from_y(top + VEL_H + 20.0, top), 0.0, "below clamps");
    }

    #[test]
    fn a_chord_shares_one_dynamic_and_its_neighbours_keep_theirs() {
        let mut p = Pattern::new("A", 16, 1);
        p.add_note(0, Note { key: 0, start: 4, len: 2, vel: 1.0, slide: false, nudge: 0.0 });
        p.add_note(0, Note { key: 7, start: 4, len: 2, vel: 0.8, slide: false, nudge: 0.0 });
        p.add_note(0, Note { key: 0, start: 8, len: 1, vel: 0.6, slide: false, nudge: 0.0 });
        assert!((p.start_vel(0, 4).unwrap() - 1.0).abs() < 1e-6, "the bar draws the loudest");
        p.set_start_vel(0, 4, 0.3);
        for n in p.roll(0).iter().filter(|n| n.start == 4) {
            assert!((n.vel - 0.3).abs() < 1e-6, "the whole chord moved");
        }
        assert!((p.roll(0).iter().find(|n| n.start == 8).unwrap().vel - 0.6).abs() < 1e-6,
                "the neighbour kept its own dynamic");
        assert_eq!(p.start_vel(0, 12), None, "an empty column has no bar to grab");
    }

    #[test]
    fn a_resize_drag_never_goes_below_one_step() {
        assert_eq!(len_to(4, 4), 1, "over the start column is the shortest note");
        assert_eq!(len_to(4, 2), 1, "left of the start still means one step, not a negative");
        assert_eq!(len_to(4, 9), 6, "through the pointer's column inclusive");
    }

    #[test]
    fn keys_move_transpose_and_resize_the_selected_note_and_refuse_to_leave_the_pattern() {
        let n = Note { key: 3, start: 0, len: 2, vel: 1.0, slide: false, nudge: 0.0 };
        let mv = |k| key_edit(0, n, k, 16).map(|(e, s)| (e, s));
        assert_eq!(mv(RollKey::Up), Some((RollEdit::Move { channel: 0, start: 0, key: 3, to_start: 0, to_key: 4 }, Some((0, 4)))));
        assert_eq!(mv(RollKey::OctaveDown).unwrap().1, Some((0, -9)));
        assert_eq!(mv(RollKey::Left), None, "already at step 0: refused, not wrapped");
        assert_eq!(mv(RollKey::Right).unwrap().1, Some((1, 3)));
        assert_eq!(mv(RollKey::Longer).unwrap().0, RollEdit::Resize { channel: 0, start: 0, key: 3, len: 3 });
        assert_eq!(key_edit(0, Note { len: 1, ..n }, RollKey::Shorter, 16), None, "one step is the shortest");
        assert_eq!(key_edit(0, Note { start: 14, ..n }, RollKey::Longer, 16), None, "may not run past the pattern");
        assert_eq!(key_edit(0, Note { key: 127, ..n }, RollKey::Up, 16), None);
        assert_eq!(mv(RollKey::Delete), Some((RollEdit::Remove { channel: 0, start: 0, key: 3 }, None)));
    }

    #[test]
    fn shift_is_read_from_the_key_event_and_other_modifiers_leave_the_key_alone() {
        use egui::{Key, Modifiers};
        assert_eq!(roll_key(Key::ArrowRight, Modifiers::NONE), Some(RollKey::Right));
        assert_eq!(roll_key(Key::ArrowRight, Modifiers::SHIFT), Some(RollKey::Longer));
        assert_eq!(roll_key(Key::ArrowLeft, Modifiers::SHIFT), Some(RollKey::Shorter));
        assert_eq!(roll_key(Key::ArrowUp, Modifiers::SHIFT), Some(RollKey::OctaveUp));
        assert_eq!(roll_key(Key::Backspace, Modifiers::NONE), Some(RollKey::Delete));
        assert_eq!(roll_key(Key::ArrowUp, Modifiers::CTRL), None);
        assert_eq!(roll_key(Key::A, Modifiers::NONE), None);
    }

    #[test]
    fn a_move_lands_behind_the_pointer_by_where_it_was_grabbed_and_inside_the_pattern() {
        assert_eq!(move_target(1, 12, 5, 16), (11, 5));
        assert_eq!(move_target(3, 1, 5, 16), (0, 5), "cannot start before step 0");
        assert_eq!(move_target(0, 40, 5, 16), (15, 5), "nor past the last step");
    }

    #[test]
    fn the_readout_names_cells_notes_and_the_selection() {
        let p = pattern_with_note(); // key 3, steps 4-6, vel 1
        let mut st = RollState { last_len: 2, ..Default::default() };
        st.hover = Some(RollHit::Empty { key: 0, step: 0 });
        let r = readout(&st, &p, 0).unwrap();
        assert!(r.contains("step 1") && r.contains("a note of 2"), "{r}");
        st.hover = Some(RollHit::Body { start: 4, key: 3 });
        let r = readout(&st, &p, 0).unwrap();
        assert!(r.contains("steps 5–7 (3)") && r.contains("vel 100%"), "{r}");
        st.hover = None;
        assert_eq!(readout(&st, &p, 0), None);
        st.selected = Some((4, 3));
        assert!(readout(&st, &p, 0).unwrap().starts_with("selected: "));
    }

    #[test]
    fn the_view_never_mutates_the_pattern() {
        // Same pin as the grid's: an edit with no representation would be
        // invisible to a collaborator. The pure helpers take &Pattern and
        // the widget returns values; this test keeps that a promise.
        let p = pattern_with_note();
        let before = p.clone();
        let _ = hit(egui::vec2(LABEL_W + 5.0 * CELL, KEY_H * 9.5), &p, 0, 12, 25);
        let _ = len_to(4, 12);
        assert_eq!(p, before);
    }
}


/// The roll driven through egui with real pointer events -- the gestures live
/// in the widget, so pure helpers alone cannot prove them.
#[cfg(test)]
mod gesture_tests {
    use super::*;
    use egui::{Pos2, Rect, Vec2};

    const TOP: i8 = 12;
    const ROWS: usize = 25;

    struct Rig { ctx: egui::Context, p: Pattern, st: RollState, t: f64, all: Vec<RollEdit> }

    impl Rig {
        fn new() -> Rig {
            let mut p = Pattern::new("g", 16, 1);
            p.add_note(0, Note { key: 3, start: 4, len: 3, vel: 0.7, slide: false, nudge: 0.0 });   // 4-6
            p.add_note(0, Note { key: 3, start: 10, len: 1, vel: 1.0, slide: false, nudge: 0.0 });  // 10
            let mut r = Rig { ctx: egui::Context::default(), p, st: RollState::default(), t: 0.0, all: vec![] };
            // egui hit-tests against the PREVIOUS frame's widget rects, and a
            // new Area spends its first appearance on a sizing pass: a press
            // in the first frames lands on nothing (measured: 1 frame is not
            // enough; the tests that happened to act later passed anyway).
            for _ in 0..3 { r.frame(vec![], Pos2::new(1.0, 1.0)); }
            r
        }
        /// Centre of cell (step, key).
        fn at(step: f32, key: i8) -> Pos2 {
            Pos2::new(LABEL_W + (step + 0.5) * CELL, (row_of(key, TOP, ROWS).unwrap() as f32 + 0.5) * KEY_H)
        }
        /// One frame; edits are applied as a host would, and returned.
        fn frame(&mut self, events: Vec<egui::Event>, pointer: Pos2) -> Vec<RollEdit> {
            self.t += 0.05;
            let mut ev = vec![egui::Event::PointerMoved(pointer)];
            ev.extend(events);
            let input = egui::RawInput { events: ev, time: Some(self.t),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))), ..Default::default() };
            let colors = uikit_egui::PaletteColors::from_palette(&snapclone_theme::palette::DARK);
            let mut out = Vec::new();
            let (p, st) = (self.p.clone(), &mut self.st);
            let _ = self.ctx.run(input, |ctx| {
                egui::Area::new(egui::Id::new("roll")).fixed_pos(Pos2::ZERO).show(ctx, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    out = roll(ui, &p, 0, TOP, ROWS, st, None, None, KeyView::Off, &colors);
                });
            });
            for e in &out {
                match *e {
                    RollEdit::Add { channel, note } => self.p.add_note(channel, note),
                    RollEdit::Remove { channel, start, key } => { self.p.remove_note(channel, start, key); }
                    RollEdit::Resize { channel, start, key, len } => self.p.resize_note(channel, start, key, len),
                    RollEdit::ToggleSlide { channel, start, key } => self.p.toggle_note_slide(channel, start, key),
                    RollEdit::Move { channel, start, key, to_start, to_key } => { self.p.move_note(channel, start, key, to_start, to_key); }
                    RollEdit::SetVel { .. } => {}
                }
            }
            self.all.extend(out.iter().copied());
            out
        }
        fn button(&mut self, at: Pos2, b: egui::PointerButton, pressed: bool) -> Vec<RollEdit> {
            self.frame(vec![egui::Event::PointerButton { pos: at, button: b, pressed, modifiers: Default::default() }], at)
        }
        fn click(&mut self, at: Pos2, b: egui::PointerButton) -> Vec<RollEdit> {
            let mut e = self.button(at, b, true);
            e.extend(self.button(at, b, false));
            e
        }
    }
    use egui::PointerButton::{Primary, Secondary};

    #[test]
    fn a_click_selects_a_note_and_does_not_delete_it() {
        let mut r = Rig::new();
        let e = r.click(Rig::at(5.0, 3), Primary);
        assert!(e.is_empty(), "{e:?}");
        assert_eq!(r.st.selected, Some((4, 3)));
        assert_eq!(r.p.roll(0).len(), 2);
    }

    #[test]
    fn right_click_deletes() {
        let mut r = Rig::new();
        r.click(Rig::at(5.0, 3), Primary);
        let e = r.click(Rig::at(5.0, 3), Secondary);
        assert_eq!(e, vec![RollEdit::Remove { channel: 0, start: 4, key: 3 }]);
        assert_eq!(r.st.selected, None);
    }

    #[test]
    fn a_click_on_empty_places_a_note_of_the_last_length_drawn() {
        let mut r = Rig::new();
        // draw one out to 3 steps: press at step 0, drag to step 2, release
        r.button(Rig::at(0.0, 7), Primary, true);
        r.frame(vec![], Rig::at(1.0, 7));
        r.frame(vec![], Rig::at(2.0, 7));
        r.button(Rig::at(2.0, 7), Primary, false);
        assert_eq!(r.p.note_at(0, 7, 0).map(|n| n.len), Some(3), "{:?}", r.all);
        assert_eq!(r.st.last_len, 3);
        r.t += 1.0; // well past double-click time
        let e = r.click(Rig::at(12.0, 5), Primary);
        assert_eq!(e, vec![RollEdit::Add { channel: 0, note: Note { key: 5, start: 12, len: 3, vel: 1.0, slide: false, nudge: 0.0 } }]);
    }

    #[test]
    fn a_dragged_note_moves_on_drop_and_crosses_other_notes_without_deleting_them() {
        let mut r = Rig::new();
        // grab note 4-6 (key 3) one step into its body, drag across the note at 10
        r.button(Rig::at(5.0, 3), Primary, true);
        for step in [6.0, 8.0, 10.0, 11.0, 12.0] {
            let e = r.frame(vec![], Rig::at(step, 5));
            assert!(e.is_empty(), "nothing on the wire mid-drag: {e:?}");
            assert_eq!(r.p.roll(0).len(), 2, "the note at 10 survives being dragged over");
        }
        assert!(matches!(r.st.gesture, Some(Gesture::Move { to: (11, 5), .. })), "{:?}", r.st.gesture);
        let e = r.button(Rig::at(12.0, 5), Primary, false);
        // grabbed one step in, so it lands one step behind the pointer: 11
        assert_eq!(e, vec![RollEdit::Move { channel: 0, start: 4, key: 3, to_start: 11, to_key: 5 }]);
        assert_eq!(r.p.note_at(0, 5, 11), Some(Note { key: 5, start: 11, len: 3, vel: 0.7, slide: false, nudge: 0.0 }));
        assert!(r.p.note_at(0, 3, 10).is_some());
        assert_eq!(r.st.selected, Some((11, 5)));
    }

    #[test]
    fn double_click_toggles_slide_but_not_on_a_note_the_first_click_just_made() {
        let mut r = Rig::new();
        r.click(Rig::at(5.0, 3), Primary);
        let e = r.click(Rig::at(5.0, 3), Primary);
        assert_eq!(e, vec![RollEdit::ToggleSlide { channel: 0, start: 4, key: 3 }], "{:?}", r.all);
        r.t += 1.0;
        // a quick double click on EMPTY: the first adds, the second must not slide it
        r.click(Rig::at(13.0, 0), Primary);
        let e = r.click(Rig::at(13.0, 0), Primary);
        assert!(e.is_empty(), "{e:?}");
        assert!(!r.p.note_at(0, 0, 13).unwrap().slide);
    }
}
