//! holdmenu.rs — logscope's charge-hold fan: press, WAIT while the border fills, slide, release.
//!
//! The gesture itself now lives in `touchkit::charge`, shared across the portfolio; this file is
//! the logscope-flavoured shell — the drawing, the geometry, and the duration choice.
//!
//! ## Why logscope charges for a full second when voiceterm charges for a quarter
//!
//! Same widget, opposite correct answers, and the deciding question is not screen size: it is what
//! the control costs when it fires by accident. In a quick pane-flipper the fan IS navigation —
//! used constantly, and a long charge makes the whole UI feel stuck. Here the fan changes settings
//! on runs that continue for hours or days: a worker's dilation, a mode, a target. A mis-swipe
//! that silently retunes one of those is a fault nobody notices until the output is wrong, long
//! after the gesture is forgotten and with no way to connect the two. A second of deliberate hold
//! is cheap against that, and it is long enough to be INTERRUPTED on purpose, which is the actual
//! feature — friction for its own sake would just be slow.
//!
//! The border animation is not decoration either. A control that fires after an invisible delay
//! feels broken twice: once when nothing happens, and again when something happens after you gave
//! up. The filling ring says "still going, let go to abort" for the whole second, and it visibly
//! DIES when the finger starts scrolling, which is how the scroll rule teaches itself.
//!
//! Ported from voiceterm's GestureHost/SlidePopup pair, whose header records two broken attempts
//! before the working one: popups tried to grab input themselves and raced the button. The rule
//! carried over is that ONE event source owns the gesture — the button's Response is the only
//! reader of the pointer, and the fan is painted, never interacted with.

pub use touchkit::{Charge, ChargeConfig, ChargeOutcome};

/// logscope's charge: a deliberate second. See the module header for why this differs per app.
pub const CHARGE: ChargeConfig = ChargeConfig::deliberate();

/// Height of one fan row, and the minimum touch target (mobilelayout's 44pt rule applies to the
/// fan too — options a finger cannot land on are options that do not exist).
pub const ROW_PT: f32 = 44.0;

/// Geometry for the fan. The gesture is `touchkit::Charge`; this is the part that knows where the
/// rows are, which is the part that differs per app.
pub struct Fan;

impl Fan {
    /// The fan's rows stack UPWARD from the button, because a toolbar lives at the bottom of a
    /// phone and a downward fan would open off-screen — the same out-of-bounds class
    /// `mobilelayout::dialog_width` fixes for dialogs.
    ///
    /// Returns the option index under `dy` (pointer y minus button top, negative = above), or None
    /// when the finger is outside the stack.
    pub fn row_at(dy: f32, rows: usize) -> Option<usize> {
        if rows == 0 || dy >= 0.0 { return None; }
        let i = ((-dy) / ROW_PT).floor() as usize;
        // The topmost row is the LAST option: rows stack upward, so index 0 is nearest the button
        // — the option the finger reaches first with the least movement, which should be the one
        // used most.
        if i < rows { Some(i) } else { None }
    }
}

/// Draw a hold-to-fan button and run its gesture. `state` persists across frames (one per button).
///
/// Returns `Some(None)` for a tap (fire the primary action) and `Some(Some(i))` for a committed
/// option. The fan is PAINTED, never interacted with: this button's `Response` is the only reader
/// of the pointer, which is the whole architectural point inherited from voiceterm's version —
/// two widgets contending for one finger is a race in any toolkit.
pub fn hold_button(ui: &mut egui::Ui, state: &mut Charge, icon: &str, options: &[String])
    -> Option<Option<usize>>
{
    let size = egui::vec2(ROW_PT.min(ui.available_width()), ROW_PT.min(ui.available_height().max(28.0)));
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let down = resp.is_pointer_button_down_on();
    let pointer = ui.ctx().pointer_interact_pos();

    // egui gives no press timestamp, so the caller's frame clock is the clock: accumulate real
    // frame time while the button is held. Using input().time deltas rather than a wall clock
    // keeps the hold honest when the app is throttled — a backgrounded app does not silently
    // accumulate 400ms of "hold".
    let dt_ms = (ui.input(|i| i.stable_dt) * 1000.0) as u64;
    let id = resp.id.with("holdfan_ms");
    let mut elapsed: u64 = ui.data(|d| d.get_temp(id)).unwrap_or(0);
    let start_id = resp.id.with("holdfan_start");
    let mut start: Option<egui::Pos2> = ui.data(|d| d.get_temp(start_id));
    if down {
        elapsed = elapsed.saturating_add(dt_ms.max(1));
        if start.is_none() { start = pointer; }
    }
    let travel = match (start, pointer) { (Some(a), Some(b)) => (b - a).length(), _ => 0.0 };
    let over = pointer.and_then(|p| {
        // Horizontal leash: a finger that has slid off the column is no longer choosing from it.
        if (p.x - rect.center().x).abs() > rect.width() * 2.0 { return None; }
        Fan::row_at(p.y - rect.top(), options.len())
    });

    let out = state.feed(CHARGE, down, elapsed, travel, over);
    if !down { elapsed = 0; start = None; }
    ui.data_mut(|d| {
        d.insert_temp(id, elapsed);
        match start {
            Some(p) => { d.insert_temp(start_id, p); }
            None => { d.remove::<egui::Pos2>(start_id); }
        }
    });

    // THE CHARGING BORDER. An arc that closes over CHARGE.hold_ms, so the wait is visible for its
    // whole duration and letting go is an obvious abort. It vanishes the instant the press is
    // disqualified as a scroll (level() returns 0 then), which is how the scroll rule teaches
    // itself without a word of documentation.
    let p = ui.painter();
    let lvl = state.level();
    if lvl > 0.0 && !state.is_open() {
        let steps = 40usize;
        let lit = ((steps as f32) * lvl).ceil() as usize;
        let r = rect.shrink(3.0);
        let peri = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
        // walk the perimeter as four segments; light the fraction travelled
        for i in 0..lit.min(steps) {
            let t = i as f32 / steps as f32;
            let seg = (t * 4.0).floor() as usize % 4;
            let ft = t * 4.0 - seg as f32;
            let a = peri[seg];
            let b = peri[(seg + 1) % 4];
            let q = a + (b - a) * ft;
            p.circle_filled(q, 1.4, crate::theme::accent().gamma_multiply(0.55 + 0.45 * lvl));
        }
    }
    let face = if state.is_open() { crate::theme::accent() }
               else if lvl > 0.0 { crate::theme::accent().gamma_multiply(0.5 + 0.5 * lvl) }
               else { crate::theme::DIM() };
    p.text(rect.center(), egui::Align2::CENTER_CENTER, icon,
           egui::FontId::proportional(16.0), face);
    // the ⋯ affordance: without it nobody discovers the fan, and an undiscoverable gesture is a
    // feature that does not exist.
    if !options.is_empty() && !state.is_open() {
        p.text(rect.center_bottom() - egui::vec2(0.0, 4.0), egui::Align2::CENTER_BOTTOM, "⋯",
               egui::FontId::proportional(9.0), crate::theme::DIM().gamma_multiply(0.7));
    }

    if let ChargeOutcome::Open(hi) = out {
        // Painted on the foreground layer so it is not clipped by the toolbar's own rect — the fan
        // is taller than the row it grows out of, by design.
        let paint = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Foreground, resp.id.with("holdfan_layer")));
        for (i, label) in options.iter().enumerate() {
            let top = rect.top() - ROW_PT * (i as f32 + 1.0);
            let r = egui::Rect::from_min_size(egui::pos2(rect.left(), top),
                                              egui::vec2(rect.width().max(140.0), ROW_PT));
            let on = hi == Some(i);
            paint.rect_filled(r, 6.0, if on { crate::theme::accent().gamma_multiply(0.30) }
                                      else { crate::theme::DIM().gamma_multiply(0.14) });
            paint.text(r.left_center() + egui::vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, label,
                       egui::FontId::proportional(13.0),
                       if on { crate::theme::accent() } else { crate::theme::DIM() });
        }
        ui.ctx().request_repaint();
    } else if lvl > 0.0 {
        // keep the border animating while charging, with no further input
        ui.ctx().request_repaint();
    }

    match out {
        ChargeOutcome::Tap => Some(None),
        ChargeOutcome::Commit(i) => Some(Some(i)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The gesture's own tests (charge duration, the scroll rule, lost frames) live with the
    // machine in touchkit::charge. What is logscope's here is the GEOMETRY.

    #[test]
    fn the_fan_stacks_UPWARD_with_the_nearest_row_first() {
        // Rows go up because a toolbar sits at the bottom of a phone; index 0 is nearest the
        // button, so the most-used option needs the least travel.
        assert_eq!(Fan::row_at(-1.0, 3), Some(0));
        assert_eq!(Fan::row_at(-(ROW_PT + 1.0), 3), Some(1));
        assert_eq!(Fan::row_at(-(ROW_PT * 2.0 + 1.0), 3), Some(2));
        assert_eq!(Fan::row_at(-(ROW_PT * 3.0 + 1.0), 3), None, "past the top of the stack");
        assert_eq!(Fan::row_at(5.0, 3), None, "below the button is not the fan");
        assert_eq!(Fan::row_at(-10.0, 0), None);
    }

    #[test]
    fn logscope_charges_DELIBERATELY_and_that_is_the_point() {
        // If this ever quietly becomes quick(), a mis-swipe can retune a multi-day run and nobody
        // will connect the two. The constant is the guard.
        assert_eq!(CHARGE.hold_ms, 1000);
        let mut c = Charge::new();
        assert_eq!(c.feed(CHARGE, true, 400, 0.0, None), ChargeOutcome::Idle);
        assert!(c.level() > 0.3 && c.level() < 0.5, "and the border shows the wait: {}", c.level());
    }
}
