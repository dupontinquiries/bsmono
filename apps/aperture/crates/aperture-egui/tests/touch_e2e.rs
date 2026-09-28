//! A finger on the real `view::frame`, through egui_kittest: the gesture
//! module is unit-tested alone; this proves the view wires it — that a drag
//! scrolls instead of selecting, even with the simulated mouse events
//! egui-winit sends alongside every touch.

use aperture_egui::state::AppState;
use aperture_egui::view;
use egui::{pos2, Event, PointerButton, Pos2, TouchDeviceId, TouchId, TouchPhase};
use egui_kittest::Harness;

fn touch(phase: TouchPhase, p: Pos2) -> Event {
    Event::Touch { device_id: TouchDeviceId(1), id: TouchId(1), phase, pos: p, force: None }
}

/// What egui-winit sends for one finger: the Touch event AND a simulated
/// pointer event at the same place.
fn finger(h: &mut Harness<'_, AppState>, phase: TouchPhase, p: Pos2) {
    let evs = &mut h.input_mut().events;
    evs.push(touch(phase, p));
    evs.push(Event::PointerMoved(p));
    match phase {
        TouchPhase::Start => evs.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Default::default() }),
        TouchPhase::End => evs.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Default::default() }),
        _ => {}
    }
}

fn harness() -> Harness<'static, AppState> {
    let mut s = AppState::new(None, false);
    s.caret.style = aperture_egui::camera::CaretStyle::Solid;
    let text: String = (0..300).map(|i| format!("let line_{i} = {i};\n")).collect();
    s.editor = aperture_core::editor::Editor::from_str(&text);
    let mut h = Harness::new_ui_state(|ui, state: &mut AppState| view::frame(ui, state), s);
    h.set_size(egui::Vec2::new(400.0, 700.0));
    h.run_steps(3);
    h
}

#[test]
fn a_finger_drag_scrolls_the_page_and_selects_nothing() {
    let mut h = harness();
    let before = h.state().cam.scroll_y.pos;
    finger(&mut h, TouchPhase::Start, pos2(200.0, 500.0));
    h.step();
    for k in 1..=30 {
        finger(&mut h, TouchPhase::Move, pos2(200.0, 500.0 - 10.0 * k as f32));
        h.step();
    }
    finger(&mut h, TouchPhase::End, pos2(200.0, 200.0));
    h.step();
    let s = h.state();
    assert!(s.cam.scroll_y.pos - before > 200.0, "scrolled {} -> {}", before, s.cam.scroll_y.pos);
    assert_eq!(s.editor.sel.anchor, s.editor.sel.head, "no selection was dragged out");
    // And the caret did not jump to where the finger landed: a scroll is
    // not a click. (A drag that scrolls keeps the finger over the same
    // character, so "no selection" alone would pass even with the mouse
    // path live — this is the assertion that catches that.)
    assert_eq!(s.editor.sel.head, 0, "the caret stayed where it was");
}

#[test]
fn a_tap_moves_the_caret_to_the_tapped_line() {
    let mut h = harness();
    let p = pos2(150.0, 200.0);
    finger(&mut h, TouchPhase::Start, p);
    h.step();
    finger(&mut h, TouchPhase::End, p);
    h.step();
    let s = h.state();
    let line = s.editor.buffer.char_to_pos(s.editor.sel.head).line;
    assert!(line > 3, "the caret left line 0 for the tapped row, got line {line}");
}
