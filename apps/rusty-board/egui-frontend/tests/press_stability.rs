//! Taps on the real screen (`Feel::frame`), driven with synthetic touch events: a tap must
//! not move the keyboard under the finger, and a key must not also type a space.
//!
//! Both are end-to-end on purpose. The row reflow came from `Adaptive::detect` reading
//! touch per frame, two crates away from the keys; the double-fire came from a space zone
//! the demo screen laid over the keys. Neither is visible to a test of `keyboard_ui` alone
//! -- a probe of `keyboard_ui` at rest was clean while the screen was not.
use rusty_board_egui::demo_app;

type Glyphs = Vec<(String, egui::Rect)>;

/// Tablet width on purpose: 600-1024pt is where pointer and touch sizes differed.
const SCREEN: egui::Vec2 = egui::vec2(700.0, 340.0);

struct Screen {
    ctx: egui::Context,
    app: demo_app::Feel,
    t: f64,
}

impl Screen {
    fn new() -> Screen {
        let mut s = Screen { ctx: egui::Context::default(), app: demo_app::Feel::new(), t: 0.0 };
        for _ in 0..30 {
            s.frame(vec![]); // past the reveal
        }
        s
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> Glyphs {
        self.t += 0.03;
        let input = egui::RawInput {
            events,
            time: Some(self.t),
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
            ..Default::default()
        };
        let app = &mut self.app;
        let out = self.ctx.run(input, |ctx| app.frame(ctx));
        uikit_egui::layout_probe::texts(&out.shapes).into_iter().map(|t| (t.text.clone(), t.rect)).collect()
    }

    /// A finger tap at `at`, the way a touchscreen reports it: touch + emulated pointer.
    fn tap(&mut self, at: egui::Pos2) -> Vec<Glyphs> {
        let touch = |phase| egui::Event::Touch {
            device_id: egui::TouchDeviceId(1),
            id: egui::TouchId(1),
            phase,
            pos: at,
            force: None,
        };
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let mut seen = vec![self.frame(vec![touch(egui::TouchPhase::Start), egui::Event::PointerMoved(at), button(true)])];
        seen.push(self.frame(vec![touch(egui::TouchPhase::Move)]));
        seen.push(self.frame(vec![touch(egui::TouchPhase::End), button(false), egui::Event::PointerGone]));
        for _ in 0..6 {
            seen.push(self.frame(vec![]));
        }
        seen
    }
}

/// Key glyphs only: right of the layout rail.
fn keys(g: &Glyphs) -> Glyphs {
    g.iter().filter(|(_, r)| r.left() > rusty_board_egui::layout_rail::RAIL_W + 2.0).cloned().collect()
}

/// Pressing and releasing a key moves no OTHER key. (The pressed key's own glyphs and
/// its magnifier are excluded: those are allowed to change.)
#[test]
fn a_tap_moves_no_other_key() {
    let mut s = Screen::new();
    let rest = keys(&s.frame(vec![]));
    assert!(rest.len() > 20, "the scan found {} glyphs", rest.len());
    // A top-row key, so every other row is below it -- the rows that used to move.
    let top = rest.iter().map(|(_, r)| r.top()).fold(f32::MAX, f32::min);
    let (label, at) = rest.iter().find(|(_, r)| (r.top() - top).abs() < 8.0).cloned().unwrap();
    let pressed_key = egui::Rect::from_center_size(at.center(), egui::vec2(60.0, 50.0));
    let mut worst = (0.0f32, String::new());
    for (i, frame) in s.tap(at.center()).iter().enumerate() {
        for (txt, r) in keys(frame) {
            if pressed_key.contains(r.center()) || txt == label {
                continue;
            }
            // The same glyph at rest, nearest by position (labels repeat: "!" is a hint on
            // two keys).
            let Some((_, r0)) = rest
                .iter()
                .filter(|(t, _)| *t == txt)
                .min_by(|a, b| (a.1.center() - r.center()).length().total_cmp(&(b.1.center() - r.center()).length()))
            else {
                continue;
            };
            let d = (r0.center() - r.center()).length();
            if d > worst.0 {
                worst = (d, format!("frame {i}: {txt:?} {r0:?} -> {r:?}"));
            }
        }
    }
    assert!(worst.0 < 0.5, "tapping {label:?} moved another key by {:.1}pt ({})", worst.0, worst.1);
}

/// No key on the bottom row types a space it does not carry. The report: "hitting delete
/// gives delete plus space". Every board in the rail, every bottom-row key.
#[test]
fn no_bottom_row_key_also_types_a_space() {
    let mut s = Screen::new();
    let mut checked = 0;
    // Walk the boards by tapping the rail's cells, top to bottom -- and re-select before
    // every key, because some bottom-row keys (?123, ABC) switch the board, and the next
    // tap would otherwise land on a different board's key at that spot.
    let select = |s: &mut Screen, b: usize| -> Glyphs {
        let rail = rusty_board_egui::layout_rail::RAIL_W * 0.5 + 8.0; // + CentralPanel margin
        let y = 8.0 + rusty_board_egui::layout_rail::CHEVRON_H + rusty_board_egui::layout_rail::CELL * (b as f32 + 0.5);
        s.tap(egui::pos2(rail, y));
        let g = keys(&s.frame(vec![]));
        let bottom = g.iter().map(|(_, r)| r.center().y).fold(f32::MIN, f32::max);
        let mut row: Glyphs = g.into_iter().filter(|(_, r)| (r.center().y - bottom).abs() < 10.0).collect();
        row.sort_by(|a, b| a.1.center().x.total_cmp(&b.1.center().x));
        row
    };
    for b in 0..9usize {
        let n = select(&mut s, b).len();
        for i in 0..n {
            let row = select(&mut s, b);
            let Some((label, r)) = row.get(i).cloned() else { break };
            let before = s.app.typed().to_string();
            s.tap(r.center());
            let after = s.app.typed().to_string();
            let added: String = if after.len() >= before.len() && after.starts_with(&before) {
                after[before.len()..].to_string()
            } else {
                // A backspace: what is left must be a prefix of what was there.
                assert!(before.starts_with(&after), "board {b}: {label:?} changed the text {before:?} -> {after:?}");
                String::new()
            };
            assert!(!label.contains('\u{0}') && !label.contains("act:"), "board {b}: a raw action token was painted: {label:?}");
            // The spacebar has no label; its arrow-swipe hints are the glyphs around it.
            let is_space_key = label.trim().is_empty()
                || label.to_lowercase().contains("space")
                || ["␣", "←", "→", "↑", "↓"].contains(&label.as_str());
            if !is_space_key {
                assert!(!added.contains(' '), "board {b}: tapping {label:?} typed {added:?} -- a space came with it");
            }
            checked += 1;
        }
    }
    assert!(checked >= 15, "only {checked} bottom-row keys were tapped");
}
