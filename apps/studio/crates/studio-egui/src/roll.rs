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
//! the finger meant it. Click a note to remove it; grab its right edge to
//! resize it later; right-click toggles slide. Velocity is edited elsewhere
//! (a lane, later) — a pointer has no pressure axis worth pretending about.

use studio_live::{Note, Pattern};

/// Column width — [`crate::grid::CELL`], so a roll column sits exactly under
/// the grid column it shares a step with.
pub const CELL: f32 = crate::grid::CELL;
/// Row height. Shorter than a grid cell: a useful roll shows two octaves
/// plus, and 25 rows of 22px would not fit a phone viewport.
pub const KEY_H: f32 = 14.0;
/// The octave-label gutter.
pub const LABEL_W: f32 = 44.0;
/// How close to a note's right edge counts as grabbing the edge.
pub const EDGE_W: f32 = 6.0;

/// What the roll asks the host to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RollEdit {
    Add { channel: usize, note: Note },
    Remove { channel: usize, start: u32, key: i8 },
    Resize { channel: usize, start: u32, key: i8, len: u32 },
    ToggleSlide { channel: usize, start: u32, key: i8 },
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
            top: i8, rows: usize, drag: &mut Option<NoteDrag>,
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

    // ── notes ───────────────────────────────────────────────────────────
    for n in pattern.roll(channel) {
        let Some(row) = row_of(n.key, top, rows) else { continue };
        if n.start >= pattern.steps { continue; } // hidden by a shortened pattern
        let y = rect.top() + row as f32 * KEY_H;
        let x = rect.left() + LABEL_W + n.start as f32 * CELL;
        let w = (n.len.max(1) as f32 * CELL - 2.0).max(4.0);
        let nr = egui::Rect::from_min_size(egui::pos2(x + 1.0, y + 1.5),
                                           egui::vec2(w, KEY_H - 3.0));
        // Velocity as brightness: a soft note reads soft. Floor at 0.35 so
        // a near-silent note is still findable to delete.
        let col = c.accent.gamma_multiply(0.35 + 0.65 * n.vel.clamp(0.0, 1.0));
        painter.rect_filled(nr, 2.0, col);
        if n.slide {
            // The slide marker: a small notch on the note's leading edge,
            // where the glide actually happens.
            painter.line_segment([nr.left_top(), egui::pos2(nr.left() + 4.0, nr.center().y)],
                                 egui::Stroke::new(1.5, c.text));
            painter.line_segment([egui::pos2(nr.left() + 4.0, nr.center().y), nr.left_bottom()],
                                 egui::Stroke::new(1.5, c.text));
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
    if let Some(pos) = resp.interact_pointer_pos() {
        let rel = pos - rect.min;
        let here = hit(rel, pattern, channel, top, rows);
        if resp.drag_started() {
            match here {
                Some(RollHit::Empty { key, step }) => {
                    // Draw the note out in one gesture: add it at a step,
                    // and let the rest of the drag be its resize.
                    edits.push(RollEdit::Add { channel, note: Note {
                        key, start: step, len: 1, vel: 1.0, slide: false } });
                    *drag = Some(NoteDrag { start: step, key, last_len: 1 });
                }
                Some(RollHit::Edge { start, key }) => {
                    let len = pattern.note_at(channel, key, start).map(|n| n.len).unwrap_or(1);
                    *drag = Some(NoteDrag { start, key, last_len: len });
                }
                _ => {}
            }
        }
        if let Some(d) = drag {
            if rel.x >= LABEL_W {
                let step_under = (((rel.x - LABEL_W) / CELL) as u32).min(pattern.steps.saturating_sub(1));
                let len = len_to(d.start, step_under);
                if len != d.last_len {
                    d.last_len = len;
                    edits.push(RollEdit::Resize { channel, start: d.start, key: d.key, len });
                }
            }
        } else if resp.clicked() {
            match here {
                Some(RollHit::Empty { key, step }) => {
                    edits.push(RollEdit::Add { channel, note: Note {
                        key, start: step, len: 1, vel: 1.0, slide: false } });
                }
                Some(RollHit::Body { start, key }) | Some(RollHit::Edge { start, key }) => {
                    edits.push(RollEdit::Remove { channel, start, key });
                }
                None => {}
            }
        }
        if resp.secondary_clicked() {
            if let Some(RollHit::Body { start, key }) | Some(RollHit::Edge { start, key }) = here {
                edits.push(RollEdit::ToggleSlide { channel, start, key });
            }
        }
    }
    if resp.drag_stopped() || (!resp.dragged() && !resp.is_pointer_button_down_on()) {
        *drag = None;
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern_with_note() -> Pattern {
        let mut p = Pattern::new("A", 16, 2);
        p.add_note(0, Note { key: 3, start: 4, len: 3, vel: 1.0, slide: false });
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
        p.add_note(0, Note { key: 0, start: 4, len: 2, vel: 1.0, slide: false });
        p.add_note(0, Note { key: 7, start: 4, len: 2, vel: 0.8, slide: false });
        p.add_note(0, Note { key: 0, start: 8, len: 1, vel: 0.6, slide: false });
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
