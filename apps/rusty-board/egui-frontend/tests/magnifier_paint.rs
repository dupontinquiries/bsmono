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

    // The popup is gone: nothing is drawn at the old magnifier size, so no neighbouring key or
    // swipe label is covered. The press shows as a pulse ON the key instead.
    let texts = uikit_egui::layout_probe::texts(&out.shapes);
    let big: Vec<_> = texts.iter().filter(|t| (t.font_pt - MAGNIFIER_PT).abs() < 2.0).collect();
    assert!(big.is_empty(), "a magnifier bubble is back: {:?}", big.iter().map(|t| (t.text.clone(), t.font_pt)).collect::<Vec<_>>());
    // The pulse animates in over a few frames, so run a few more with the pointer still down.
    let mut last = out;
    for k in 1..=6 {
        let mut held = input.clone();
        held.time = Some(1000.0 + k as f64 * 0.03);
        last = ctx.run(held, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = keyboard_ui(ui, board.current(), &uikit_egui::Adaptive::detect(ctx), "system");
            });
        });
    }
    let rings = last
        .shapes
        .iter()
        .filter(|s| matches!(&s.shape, egui::epaint::Shape::Rect(r) if r.stroke.width >= 1.4 && r.stroke.width <= 1.6))
        .count();
    assert!(rings >= 1, "no pulse ring drawn on the pressed key");
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
        // The UNIFORM inset, which is what this test was written for. With the reach
        // polygon on (now the default) the bottom row keeps nearly the full width by
        // design, so the board's overall span barely moves — correct, and measured by
        // `the_thumb_arc_widens_the_rows_nearest_the_anchor` instead.
        let style = Style { half: side, arc: false, ..Style::default() };
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
///
/// And they must not lie ON the keys. The full-width board used to show one bottom zone,
/// drawn over its own bottom row, and a tap on ⌫ there typed ⌫ and a space. The board now
/// occupies exactly its rows, so a full-width board has no room for a bottom zone and gets
/// none; the compacted board still gets the strip its compaction freed.
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
    assert_eq!(full.len(), 0, "a full-width board has no room under its keys for a zone, saw {}", full.len());

    let left = marks(halfboard::Side::Left);
    assert_eq!(left.len(), 1, "a compacted board gets the freed strip, saw {}", left.len());
    // The added one is the tall thin strip, not a second bottom bar.
    let strip = left.iter().max_by(|a, b| a.center().x.partial_cmp(&b.center().x).unwrap()).unwrap();
    assert!(strip.center().x > size.x * 0.6, "the strip is not on the freed side: {strip:?}");
}

/// A ripple that is computed and never painted teaches nobody anything. This drives the
/// real screen with a ripple in flight and checks a circle reaches the frame.
#[test]
fn a_delimiter_ripple_is_painted_and_then_expires() {
    let ctx = egui::Context::default();
    let mut app = demo_app::Feel::new();
    let size = egui::vec2(500.0, 300.0);
    let mut input = egui::RawInput::default();
    input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size));

    let circles = |out: &egui::FullOutput| {
        out.shapes
            .iter()
            .filter(|s| matches!(&s.shape, egui::Shape::Circle(_)))
            .count()
    };

    let before = circles(&ctx.run(input.clone(), |ctx| app.frame(ctx)));
    // A crossing just happened, at the board's bottom edge.
    app.ripples.push((egui::pos2(250.0, 260.0), 0.0));
    let during = circles(&ctx.run(input.clone(), |ctx| app.frame(ctx)));
    assert!(during > before, "no ripple was painted: {before} → {during}");

    // Past its window it is gone, and drained rather than accumulating forever.
    app.ripples = vec![(egui::pos2(250.0, 260.0), -10_000.0)];
    let after = circles(&ctx.run(input, |ctx| app.frame(ctx)));
    assert_eq!(after, before, "an expired ripple was still drawn");
    assert!(app.ripples.is_empty(), "expired ripples are not drained");
}

/// The reach polygon must produce a GRADIENT — rows near the anchor wider than rows far
/// from it. A uniform inset would pass a "keys moved" test and miss the whole point, which
/// is why this measures per-row width rather than position.
#[test]
fn the_thumb_arc_widens_the_rows_nearest_the_anchor() {
    let board = Board::new();
    let size = egui::vec2(720.0, 300.0);

    let row_spans = |style: Style| -> Vec<(f32, f32)> {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size));
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
        // Group the painted glyphs into rows by y, then take each row's horizontal span.
        // CLUSTERED, not bucketed: fixed 10pt buckets split one row's glyphs across two
        // buckets whenever the row's centre landed near a bucket edge (the label and a
        // key's corner hint sit a few points apart), and a one-glyph "row" made the flat
        // board's spread look like 450pt. A new row starts at a vertical gap larger than
        // any within-key offset and smaller than the row pitch.
        let mut ts: Vec<egui::Rect> = uikit_egui::layout_probe::texts(&out.shapes).iter().map(|t| t.rect).collect();
        ts.sort_by(|a, b| a.center().y.partial_cmp(&b.center().y).unwrap());
        let mut rows: Vec<(f32, f32, f32)> = Vec::new(); // (last y, left, right)
        for r in ts {
            match rows.last_mut() {
                Some(row) if r.center().y - row.0 < 14.0 => {
                    row.0 = r.center().y;
                    row.1 = row.1.min(r.left());
                    row.2 = row.2.max(r.right());
                }
                _ => rows.push((r.center().y, r.left(), r.right())),
            }
        }
        rows.into_iter().map(|(_, l, r)| (l, r)).collect()
    };

    let arced = row_spans(Style { half: halfboard::Side::Right, arc: true, ..Style::default() });
    assert!(arced.len() >= 3, "expected several rows, saw {}", arced.len());

    let width = |r: &(f32, f32)| r.1 - r.0;
    let first = width(&arced[0]);
    let last = width(arced.last().unwrap());
    assert!(last > first, "no gradient: top row {first:.0}pt, bottom row {last:.0}pt");

    // A uniform inset gives every row the same width — so this test can tell them apart.
    let flat = row_spans(Style { half: halfboard::Side::Right, arc: false, ..Style::default() });
    let fw: Vec<f32> = flat.iter().map(width).collect();
    let spread = fw.iter().cloned().fold(f32::MIN, f32::max) - fw.iter().cloned().fold(f32::MAX, f32::min);
    assert!(
        (last - first) > spread,
        "the arc's gradient ({:.0}pt) is no bigger than the uniform inset's ({spread:.0}pt)",
        last - first
    );
}
