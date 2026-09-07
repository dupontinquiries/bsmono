//! The magnifier must actually reach the frame — the placement test checks arithmetic, and
//! arithmetic that is never painted is a bubble nobody sees.
use rusty_board_egui::*;

#[test]
fn a_pressed_key_paints_its_magnifier_above_the_board() {
    let board = Board::new();
    let ctx = egui::Context::default();
    let size = egui::vec2(360.0, 220.0);

    let mut input = egui::RawInput::default();
    input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size));
    // Frame 1 lays the board out so the keys have rects to be pressed in.
    let _ = ctx.run(input.clone(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = keyboard_ui(ui, board.current(), &uikit_egui::Adaptive::detect(ctx), "system");
        });
    });

    // Frame 2: a pointer held down on a LETTER. At 0.75 of the height this landed on the
    // bottom row — the space bar — and got no magnifier, which is the correct behaviour and
    // a useless test. The middle of the second row is a letter on every layout here.
    let at = egui::pos2(size.x * 0.5, size.y * 0.42);
    let mut down = input.clone();
    down.events = vec![
        egui::Event::PointerMoved(at),
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ];
    let out = ctx.run(down, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = keyboard_ui(ui, board.current(), &uikit_egui::Adaptive::detect(ctx), "system");
        });
    });

    // The magnified glyph is drawn at a size no cap label uses.
    let texts = uikit_egui::layout_probe::texts(&out.shapes);
    let big: Vec<_> = texts.iter().filter(|t| (t.font_pt - MAGNIFIER_PT).abs() < 2.0).collect();
    assert_eq!(
        big.len(),
        1,
        "expected exactly one magnified glyph, saw {:?}",
        big.iter().map(|t| (t.text.clone(), t.font_pt)).collect::<Vec<_>>()
    );
    let m = big[0].rect;
    assert!(m.center().y < at.y, "the magnifier is not above the press: {m:?}");
    assert!(m.left() >= -1.0 && m.right() <= size.x + 1.0, "{m:?} left the board");
}

/// One-handed compaction must actually move the keys, and to the side asked for. The
/// geometry is unit-tested; this checks the renderer honours it, because a `Side` the
/// layout ignores is a setting that changes nothing.
#[test]
fn a_compacted_board_draws_its_keys_on_the_chosen_side() {
    let board = Board::new();
    let size = egui::vec2(500.0, 220.0);

    let draw = |side: halfboard::Side| -> Vec<egui::Rect> {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size));
        let style = Style { half: side, ..Style::default() };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = keyboard_ui_styled(
                    ui,
                    board.current(),
                    &uikit_egui::Adaptive::detect(ctx),
                    "system",
                    style,
                );
            });
        });
        uikit_egui::layout_probe::texts(&out.shapes).iter().map(|t| t.rect).collect()
    };

    let span = |rs: &[egui::Rect]| {
        let l = rs.iter().map(|r| r.left()).fold(f32::MAX, f32::min);
        let r = rs.iter().map(|r| r.right()).fold(f32::MIN, f32::max);
        (l, r)
    };

    let (fl, fr) = span(&draw(halfboard::Side::Off));
    let (ll, lr) = span(&draw(halfboard::Side::Left));
    let (rl, rr) = span(&draw(halfboard::Side::Right));

    assert!(lr < fr - 20.0, "a left-anchored board did not pull its right edge in: {lr} vs {fr}");
    assert!((ll - fl).abs() < 12.0, "a left-anchored board moved off the left edge: {ll} vs {fl}");
    assert!(rl > fl + 20.0, "a right-anchored board did not move off the left edge: {rl} vs {fl}");
    assert!(rr > lr, "both sides drew in the same place");
}
