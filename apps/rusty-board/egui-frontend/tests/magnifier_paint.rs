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

/// The space zones must reach the frame. `springboard::zones` computed them for a commit
/// with nothing drawing them, which is the failure this crate keeps producing — a strip the
/// user cannot see is not a spacebar.
#[test]
fn the_space_zones_are_painted_and_the_compacted_one_appears_beside_the_keys() {
    let size = egui::vec2(500.0, 300.0);

    // The ␣ mark is the zone's own glyph; no key label uses it.
    let marks = |side: halfboard::Side| -> Vec<egui::Rect> {
        let ctx = egui::Context::default();
        let mut app = demo_app::Feel::new();
        app.style = Style { half: side, ..app.style };
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size));
        // Two frames: the first lays the board out, the second draws against real rects.
        let _ = ctx.run(input.clone(), |ctx| app.frame(ctx));
        let out = ctx.run(input, |ctx| app.frame(ctx));
        uikit_egui::layout_probe::texts(&out.shapes)
            .into_iter()
            .filter(|t| t.text.trim() == "␣")
            .map(|t| t.rect)
            .collect()
    };

    let full = marks(halfboard::Side::Off);
    assert_eq!(full.len(), 1, "a full-width board has one space zone, saw {}", full.len());

    let left = marks(halfboard::Side::Left);
    assert_eq!(left.len(), 2, "a compacted board adds the freed strip, saw {}", left.len());
    // The added one is the tall thin strip, not a second bottom bar.
    let strip = left.iter().max_by(|a, b| a.center().x.partial_cmp(&b.center().x).unwrap()).unwrap();
    assert!(strip.center().x > size.x * 0.6, "the strip is not on the freed side: {strip:?}");
}
