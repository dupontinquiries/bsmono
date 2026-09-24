//! grid — the step-sequencer surface for `studio-live`.
//!
//! `studio-live` models a rack: a tempo, channels, and patterns storing one
//! bitmask per channel. `studio-egui` had the NLE timeline and nothing for the
//! live side, so the primary surface of the sequencer — the grid you actually
//! perform on — had no view at all.
//!
//! ## Nothing here owns the pattern
//!
//! Every interaction returns a `GridEdit` for the host to apply. That is not
//! ceremony: `studio-collab` exists because a Studio project is meant to be
//! edited by two people at once, and the README is explicit that kdenlive and
//! Audacity cannot be, "because in both the edit is a method call on a model —
//! there is nothing to send". A view that toggled a bit directly would
//! reintroduce exactly that: an edit with no representation, invisible to the
//! collaborator.
//!
//! ## Painting, not tapping
//!
//! Dragging across the grid sets a run of steps, and the value is decided by
//! the FIRST cell touched — press on an empty step and you paint on, press on a
//! filled one and you erase. Deciding per-cell instead would toggle each step
//! under the finger, so a drag across a half-filled row inverts it, which is
//! never what anyone means.
//!
//! ## The playhead column is a read, not a state
//!
//! It comes from the transport's tick and is recomputed each frame. Storing it
//! would let the grid disagree with the audio it is drawn beside, and on a
//! sequencer that disagreement is the one thing the eye catches instantly.

use studio_core::Ticks;
use studio_live::{Pattern, Tempo};

/// What the grid asks the host to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GridEdit {
    /// Set one step, with the value the drag decided.
    SetStep { channel: usize, step: u32, on: bool },
    /// Clear an entire channel row.
    ClearChannel { channel: usize },
    ToggleMute { channel: usize },
}

/// A drag in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Paint {
    /// The value being painted, decided by the first cell.
    value: Option<bool>,
    /// The last cell written, so a drag does not re-emit the same edit every
    /// frame — a sequencer at 60fps would otherwise send sixty identical edits
    /// per cell to every collaborator.
    last: Option<(usize, u32)>,
}

impl Paint {
    pub fn active(&self) -> bool { self.value.is_some() }

    /// Begin a drag on a cell whose current value is `was_set`.
    pub fn begin(&mut self, was_set: bool) { self.value = Some(!was_set); self.last = None; }

    /// Continue onto a cell. Returns the edit if this cell is new.
    pub fn touch(&mut self, channel: usize, step: u32) -> Option<GridEdit> {
        let on = self.value?;
        if self.last == Some((channel, step)) { return None; }
        self.last = Some((channel, step));
        Some(GridEdit::SetStep { channel, step, on })
    }

    pub fn end(&mut self) { self.value = None; self.last = None; }
}

/// Which step column the transport is on.
///
/// Recomputed from ticks rather than stored — see the module docs. Returns
/// `None` for a pattern with no steps, which is a valid empty state rather than
/// a reason to divide by zero.
pub fn playhead_step(tempo: &Tempo, pattern: &Pattern, now: Ticks) -> Option<u32> {
    if pattern.steps == 0 { return None; }
    let step = tempo.step_at(now);
    // Wrapped with rem_euclid so a negative tick — a count-in, or a transport
    // scrubbed before zero — lands on a real column instead of panicking or
    // showing the last step.
    Some((step.rem_euclid(pattern.steps as i64)) as u32)
}

/// Is this step on a beat boundary, for the heavier gridline?
///
/// Four steps to a beat by convention. Without the emphasis a sixteen-step row
/// is an undifferentiated stripe and you lose your place in it — which is the
/// whole reason hardware sequencers colour every fourth pad.
pub fn is_beat(step: u32) -> bool { step % 4 == 0 }

/// One step cell's square side, shared with `lanes` so a lane's columns sit
/// exactly under the grid's.
pub const CELL: f32 = 22.0;
/// The channel-name gutter width, shared with `lanes` for the same reason.
pub const LABEL_W: f32 = 96.0;

/// Draw the grid. Returns edits for the host to apply.
///
/// `selected` is the channel the piano roll edits: its row is marked, so the
/// two views of a pattern agree about which channel you are on. Where a
/// channel has piano-roll notes, a small dot sits under each step a note
/// STARTS on -- the roll's content seen from here, so switching views never
/// reveals notes you did not know were there.
pub fn grid(ui: &mut egui::Ui, pattern: &Pattern, channels: &[studio_live::Channel],
            paint: &mut Paint, playhead: Option<u32>, selected: Option<usize>,
            c: &uikit_egui::PaletteColors) -> Vec<GridEdit> {
    let mut edits = Vec::new();
    if pattern.steps == 0 || channels.is_empty() {
        ui.colored_label(c.text_weak, "No pattern loaded.");
        return edits;
    }

    let cell = CELL;
    let label_w = LABEL_W;
    let width = label_w + pattern.steps as f32 * cell;
    let height = channels.len() as f32 * cell;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height),
                                              egui::Sense::click_and_drag());
    let painter = ui.painter();

    for (ci, ch) in channels.iter().enumerate() {
        let y = rect.top() + ci as f32 * cell;
        // A muted channel dims its whole row rather than hiding it: you still
        // need to see the pattern you are about to unmute.
        let name_col = if ch.muted { c.text_weak } else { c.text };
        if selected == Some(ci) {
            painter.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.left(), y), egui::vec2(label_w - 2.0, cell)),
                                3.0, c.accent.gamma_multiply(0.25));
            painter.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.left(), y + 2.0), egui::vec2(3.0, cell - 4.0)),
                                1.0, c.accent);
        }
        painter.text(egui::pos2(rect.left() + 6.0, y + cell * 0.5),
                     egui::Align2::LEFT_CENTER, &ch.name,
                     egui::FontId::proportional(12.0), name_col);

        for s in 0..pattern.steps {
            let x = rect.left() + label_w + s as f32 * cell;
            let cr = egui::Rect::from_min_size(egui::pos2(x + 1.0, y + 1.0),
                                               egui::vec2(cell - 2.0, cell - 2.0));
            let set = pattern.is_set(ci, s);
            let fill = if set && ch.muted { c.text_weak }
                       else if set { c.accent }
                       else if is_beat(s) { c.bg_raised }
                       else { c.bg_base };
            painter.rect_filled(cr, 3.0, fill);
            if pattern.roll(ci).iter().any(|n| n.start == s) {
                painter.circle_filled(egui::pos2(cr.center().x, cr.bottom() - 3.5), 2.0,
                                      if set { c.bg_base } else { c.text_weak });
            }
            if playhead == Some(s) {
                // The playhead is a column outline, not a fill: filling would
                // hide whether the step under it is set, which is the one thing
                // you are watching for.
                painter.rect_stroke(cr, 3.0, egui::Stroke::new(1.5_f32, c.warning),
                                    egui::StrokeKind::Outside);
            }
        }
    }

    let cell_at = |pos: egui::Pos2| -> Option<(usize, u32)> {
        let rel = pos - rect.min;
        if rel.x < label_w || rel.y < 0.0 { return None; }
        let (s, ci) = (((rel.x - label_w) / cell) as u32, (rel.y / cell) as usize);
        (s < pattern.steps && ci < channels.len()).then_some((ci, s))
    };
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.drag_started() {
            // Begin where the PRESS was: egui only calls it a drag once the
            // pointer has moved past its threshold, and until c20 the paint
            // began at THAT cell -- a quick stroke skipped the cell it started
            // on and took its on/off from the wrong one.
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
            if let Some((ci, s)) = cell_at(origin) {
                paint.begin(pattern.is_set(ci, s));
                if let Some(e) = paint.touch(ci, s) { edits.push(e); }
            }
        } else if resp.clicked() && !paint.active() {
            if let Some((ci, s)) = cell_at(pos) { paint.begin(pattern.is_set(ci, s)); }
        }
        if let Some((ci, s)) = cell_at(pos) {
            if let Some(e) = paint.touch(ci, s) { edits.push(e); }
        }
    }
    if resp.drag_stopped() || (!resp.dragged() && !resp.is_pointer_button_down_on()) {
        paint.end();
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern() -> Pattern { Pattern::new("A", 16, 4) }

    #[test]
    fn a_drag_paints_one_value_decided_by_the_first_cell() {
        // Toggling per cell would invert a half-filled row under the finger,
        // which is never what anyone means.
        let mut p = pattern();
        p.set(0, 4, true);
        let mut paint = Paint::default();
        paint.begin(p.is_set(0, 0));                  // started on an EMPTY cell
        assert_eq!(paint.touch(0, 0), Some(GridEdit::SetStep { channel: 0, step: 0, on: true }));
        // Crossing a filled cell still paints ON, rather than erasing it.
        assert_eq!(paint.touch(0, 4), Some(GridEdit::SetStep { channel: 0, step: 4, on: true }));
    }

    #[test]
    fn starting_on_a_filled_cell_erases_the_run() {
        let mut p = pattern();
        p.set(1, 2, true);
        let mut paint = Paint::default();
        paint.begin(p.is_set(1, 2));
        assert_eq!(paint.touch(1, 2), Some(GridEdit::SetStep { channel: 1, step: 2, on: false }));
        assert_eq!(paint.touch(1, 3), Some(GridEdit::SetStep { channel: 1, step: 3, on: false }));
    }

    #[test]
    fn holding_still_does_not_re_emit_the_same_edit() {
        // At 60fps a held finger would send sixty identical edits per cell to
        // every collaborator.
        let mut paint = Paint::default();
        paint.begin(false);
        assert!(paint.touch(0, 0).is_some());
        assert_eq!(paint.touch(0, 0), None);
        assert_eq!(paint.touch(0, 0), None);
    }

    #[test]
    fn moving_back_onto_a_cell_after_leaving_it_edits_again() {
        // Only the IMMEDIATELY previous cell is suppressed. A drag that loops
        // back should still paint, or a circular gesture leaves holes.
        let mut paint = Paint::default();
        paint.begin(false);
        paint.touch(0, 0);
        paint.touch(0, 1);
        assert!(paint.touch(0, 0).is_some());
    }

    #[test]
    fn touching_without_a_drag_in_progress_does_nothing() {
        // A stray move event after the finger lifted must not paint.
        let mut paint = Paint::default();
        assert_eq!(paint.touch(0, 0), None);
        assert!(!paint.active());
    }

    #[test]
    fn ending_a_drag_clears_the_painted_value() {
        let mut paint = Paint::default();
        paint.begin(true);
        paint.touch(0, 0);
        paint.end();
        assert!(!paint.active());
        assert_eq!(paint.touch(0, 1), None);
    }

    #[test]
    fn the_view_never_mutates_the_pattern() {
        // studio-collab exists because a project is meant to be edited by two
        // people at once, and the README says kdenlive and Audacity cannot be
        // "because the edit is a method call on a model — there is nothing to
        // send". A view that toggled a bit directly would reintroduce that.
        let p = pattern();
        let before = p.rows.clone();
        let mut paint = Paint::default();
        paint.begin(false);
        paint.touch(0, 0);
        paint.touch(2, 7);
        assert_eq!(p.rows, before);
    }

    #[test]
    fn the_playhead_wraps_within_the_pattern() {
        let t = Tempo::default();
        let p = pattern();
        let first = playhead_step(&t, &p, Ticks::ZERO).expect("has steps");
        assert!(first < p.steps);
        // A tick well past the pattern length still lands on a real column.
        let late = playhead_step(&t, &p, Ticks(t.step_len().0 * 100)).expect("has steps");
        assert!(late < p.steps);
    }

    #[test]
    fn a_negative_tick_lands_on_a_real_column() {
        // A count-in, or a transport scrubbed before zero. rem_euclid rather
        // than % so it does not go negative and index backwards.
        let t = Tempo::default();
        let p = pattern();
        let early = playhead_step(&t, &p, Ticks(-t.step_len().0 * 3)).expect("has steps");
        assert!(early < p.steps);
    }

    #[test]
    fn an_empty_pattern_has_no_playhead_rather_than_dividing_by_zero() {
        let t = Tempo::default();
        let empty = Pattern::new("empty", 0, 2);
        assert_eq!(playhead_step(&t, &empty, Ticks::ZERO), None);
    }

    #[test]
    fn every_fourth_step_is_emphasised() {
        // Without it a sixteen-step row is an undifferentiated stripe and you
        // lose your place — the reason hardware sequencers colour every fourth
        // pad.
        assert!(is_beat(0));
        assert!(is_beat(4));
        assert!(!is_beat(1));
        assert!(!is_beat(7));
    }
}


/// The grid driven through egui with real pointer events.
#[cfg(test)]
mod gesture_tests {
    use super::*;
    use egui::{Pos2, Rect, Vec2};

    fn channels(n: usize) -> Vec<studio_live::Channel> {
        let rack = studio_live::Rack::trap_kit();
        rack.channels.into_iter().cycle().take(n).collect()
    }

    /// Press on `from`'s cell, move fast to `to` (past egui's drag threshold
    /// in one frame), release; returns every edit.
    fn stroke(p: &mut Pattern, from: (usize, u32), to: (usize, u32)) -> Vec<GridEdit> {
        let ctx = egui::Context::default();
        let chans = channels(4);
        let mut paint = Paint::default();
        let colors = uikit_egui::PaletteColors::from_palette(&snapclone_theme::palette::DARK);
        let at = |(ci, s): (usize, u32)| Pos2::new(LABEL_W + (s as f32 + 0.5) * CELL, (ci as f32 + 0.5) * CELL);
        let mut all = Vec::new();
        let mut t = 0.0;
        let mut frame = |p: &mut Pattern, ev: Vec<egui::Event>, pointer: Pos2| {
            t += 0.05;
            let mut events = vec![egui::Event::PointerMoved(pointer)];
            events.extend(ev);
            let input = egui::RawInput { events, time: Some(t),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))), ..Default::default() };
            let snapshot = p.clone();
            let mut out = Vec::new();
            let _ = ctx.run(input, |ctx| {
                egui::Area::new(egui::Id::new("grid")).fixed_pos(Pos2::ZERO).show(ctx, |ui| {
                    out = grid(ui, &snapshot, &chans, &mut paint, None, None, &colors);
                });
            });
            for e in &out { if let GridEdit::SetStep { channel, step, on } = *e { p.set(channel, step, on); } }
            all.extend(out);
        };
        for _ in 0..3 { frame(p, vec![], Pos2::new(1.0, 1.0)); } // an Area's sizing pass
        let press = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed,
                                                               modifiers: Default::default() };
        frame(p, vec![press(at(from), true)], at(from));
        frame(p, vec![], at(to));
        frame(p, vec![], at(to));
        frame(p, vec![press(at(to), false)], at(to));
        all
    }

    #[test]
    fn a_quick_stroke_paints_the_cell_it_started_on() {
        let mut p = Pattern::new("g", 16, 4);
        let e = stroke(&mut p, (1, 3), (1, 4));
        assert!(p.is_set(1, 3), "the pressed cell: {e:?}");
        assert!(p.is_set(1, 4));
    }

    #[test]
    fn a_quick_stroke_takes_its_on_or_off_from_the_cell_it_started_on() {
        // starts on an EMPTY cell, runs onto a set one: paints on, keeps it on
        let mut p = Pattern::new("g", 16, 4);
        p.set(2, 5, true);
        stroke(&mut p, (2, 4), (2, 5));
        assert!(p.is_set(2, 4) && p.is_set(2, 5), "on, decided by the start cell");
        // starts on a SET cell: erases
        let mut q = Pattern::new("g", 16, 4);
        q.set(0, 7, true);
        q.set(0, 8, true);
        stroke(&mut q, (0, 7), (0, 8));
        assert!(!q.is_set(0, 7) && !q.is_set(0, 8));
    }
}
