//! The palette, speech, the recents strip and the long press - as far as they
//! can be driven without a window or a microphone. What an utterance DOES is
//! decided in code that does not know whether the `voice` feature is on, which
//! is what makes the rule below testable here.

mod common;
use common::stopped_shell;
use stoat_core::backend::{Request, StepKind};
use stoat_core::command::Command;
use stoat_egui::palette::{Action, Heard, Palette};

fn labels(q: &str, st: &stoat_egui::shell::ShellState) -> Vec<String> {
    Palette::items(q, st).into_iter().map(|i| i.label).collect()
}

#[test]
fn what_is_offered_follows_the_session_and_the_query() {
    let (st, _) = stopped_shell();
    let idle = labels("", &st);
    assert_eq!(idle[0], "step over", "stopped: the thing you most likely want is first: {idle:?}");
    assert!(!idle.iter().any(|l| l == "interrupt"), "nothing is running, so interrupt is not offered");

    assert_eq!(labels("vit", &st)[0], "go to Vitals");
    assert_eq!(labels("p n->next", &st)[0], "print n->next", "a line that parses as a command is the first row");
    assert_eq!(labels("! info line", &st)[0], "gdb: info line");
    let dep = labels("depth1", &st);
    assert!(dep[0].contains("depth1"), "frames are searchable by function: {dep:?}");
    assert!(labels("tile", &st).iter().any(|l| l == "debug tile-viewer"), "so are binaries");
    assert!(labels("zzzzqqq", &st).is_empty(), "no match is no rows, not six bad ones");
    assert!(Palette::items("", &st).len() <= uikit_egui::command_bar::MAX_ROWS);
}

/// THE rule. Speech may navigate and inspect as heard. It may not let the
/// program run on: that discards a stop which cannot be got back, and
/// recognisers hear "continue" in sentences that did not say it.
#[test]
fn speech_navigates_at_once_but_never_resumes_without_a_confirmation() {
    let (mut st, rec) = stopped_shell();
    let mut pal = Palette::default();
    let sent = |rec: &common::Recorder| rec.0.lock().unwrap().clone();

    pal.open();
    assert_eq!(pal.heard("Go up.", &mut st), Heard::Ran(Command::Up(1)));
    assert_eq!(st.session.selected, 1, "navigation ran as heard");
    assert!(!pal.is_open(), "and the palette got out of the way");

    pal.open();
    let before = sent(&rec).len();
    assert_eq!(pal.heard("continue", &mut st), Heard::NeedsConfirm(Command::Continue));
    assert_eq!(sent(&rec).len(), before, "NOTHING was sent to gdb");
    assert!(pal.is_open(), "it waits, with the heard command in the box");
    assert_eq!(pal.query, "continue");
    for u in ["step over", "Step into.", "restart", "finish"] {
        assert!(matches!(pal.heard(u, &mut st), Heard::NeedsConfirm(_)), "{u:?}");
    }
    assert_eq!(sent(&rec).len(), before, "still nothing: every resuming utterance waited");

    // The confirmation is the ordinary row: first item for the heard text.
    pal.heard("continue", &mut st);
    let first = Palette::items(&pal.query, &st).remove(0);
    assert_eq!(first.action, Action::Session(Command::Continue));
    pal.run(&first, &mut st);
    assert_eq!(sent(&rec).last(), Some(&Request::Continue), "confirmed, it runs");

    // Not a command at all: shown as a search, so a wrong hearing is visible.
    pal.open();
    assert_eq!(pal.heard("Show me the vitals page.", &mut st), Heard::Searching);
    assert_eq!(pal.query, "vitals", "the wrapping words are dropped so the search can match");
    assert_eq!(labels(&pal.query, &st)[0], "go to Vitals", "and it is one Enter away, not run unasked");
    assert_eq!(st.page, stoat_egui::shell::PAGE_STACK, "the page did not change by itself");
}

#[test]
fn spoken_names_resolve_against_the_selected_frames_locals() {
    let (mut st, rec) = stopped_shell();
    st.select_frame(1); // depth2: n, counter, doubled
    let names: Vec<_> = st.session.locals[&1].iter().map(|v| v.name.clone()).collect();
    assert!(names.contains(&"n".to_string()), "fixture precondition: {names:?}");
    let mut pal = Palette::default();
    pal.open();
    assert_eq!(pal.heard("what is n", &mut st), Heard::Ran(Command::Print("n".into())));
    assert!(matches!(rec.0.lock().unwrap().last(), Some(Request::Evaluate { expr, frame: 1, .. }) if expr == "n"));
}

#[test]
fn the_recents_strip_reruns_by_parsing_and_never_shows_a_command_twice() {
    let (mut st, rec) = stopped_shell();
    let mut pal = Palette::default();
    let strip = |p: &Palette| p.strip.entries().iter().map(|c| c.label.clone()).collect::<Vec<_>>();
    assert_eq!(strip(&pal), ["next", "continue"], "pinned, in their short typed form");

    let print = Palette::items("p doubled", &st).remove(0);
    pal.run(&print, &mut st);
    let next = Palette::items("step over", &st).remove(0);
    pal.run(&next, &mut st);
    assert_eq!(strip(&pal), ["next", "print doubled", "continue"], "recents first; the pinned `next` moved, it did not double");

    let before = rec.0.lock().unwrap().len();
    pal.run_id("c:print doubled", &mut st);
    pal.run_id("c:next", &mut st);
    let sent = rec.0.lock().unwrap()[before..].to_vec();
    assert!(matches!(&sent[0], Request::Evaluate { expr, .. } if expr == "doubled"), "{sent:?}");
    assert_eq!(sent[1], Request::Step { kind: StepKind::Over, thread: Some(1) });
}

/// A real press, held still for 0.6 s over a source line, through the kit's
/// TouchSource and touchkit's recogniser, ends as a breakpoint request at that
/// line. And a quick tap on the same spot does not.
#[test]
fn a_long_press_on_a_source_line_asks_for_a_breakpoint_there() {
    let (mut st, rec) = stopped_shell();
    // The transcript's source path exists only where it was recorded; point
    // frame 0 at a file this test owns.
    let src = common::scratch().join("held.c");
    std::fs::write(&src, (1..=12).map(|i| format!("int line_{i} = {i};\n")).collect::<String>()).unwrap();
    st.session.frames[0].file = Some(src.clone());
    st.session.frames[0].line = Some(6);
    st.page = stoat_egui::shell::PAGE_STACK;

    let ctx = egui::Context::default();
    let c = uikit_egui::apply_palette(&ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::Night));
    let mut view = stoat_egui::View::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 760.0));
    let mut frame = |t: f64, events: Vec<egui::Event>, st: &mut stoat_egui::shell::ShellState| {
        let input = egui::RawInput { screen_rect: Some(screen), time: Some(t), events, ..Default::default() };
        #[allow(deprecated)]
        let out = ctx.run(input, |ctx| {
            #[allow(deprecated)]
            egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(10.0)).show(ctx, |ui| stoat_egui::draw(ui, st, &mut view, &c));
        });
        uikit_egui::layout_probe::texts(&out.shapes)
    };
    let mut texts = Vec::new();
    for i in 0..3 {
        texts = frame(i as f64 * 0.016, vec![], &mut st);
    }
    // The snippet is one galley; find it and aim at the row holding line 8.
    let snippet = texts.iter().find(|t| t.text.contains("int line_8 = 8;")).expect("the source snippet was drawn");
    let rows_before = snippet.text.lines().position(|l| l.contains("line_8")).unwrap();
    // split, not lines(): the galley ends with a newline, which is one more (empty) row.
    let row_h = snippet.rect.height() / snippet.text.split('\n').count() as f32;
    let pos = egui::pos2(snippet.rect.left() + 60.0, snippet.rect.top() + row_h * (rows_before as f32 + 0.5));
    let breaks = |rec: &common::Recorder| rec.0.lock().unwrap().iter().filter_map(|r| if let Request::InsertBreakpoint { location } = r { Some(location.clone()) } else { None }).collect::<Vec<_>>();
    let press = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };

    // A tap: down and up within 100 ms.
    frame(1.00, vec![egui::Event::PointerMoved(pos), press(true)], &mut st);
    frame(1.10, vec![press(false)], &mut st);
    frame(1.20, vec![], &mut st);
    assert!(breaks(&rec).is_empty(), "a tap is not a long press");

    // A hold: down, then 0.7 s of frames without moving.
    frame(2.00, vec![egui::Event::PointerMoved(pos), press(true)], &mut st);
    for i in 1..=14 {
        frame(2.00 + i as f64 * 0.05, vec![], &mut st);
    }
    frame(2.80, vec![press(false)], &mut st);
    assert_eq!(breaks(&rec), [format!("{}:8", src.display())]);
}

/// The ⌘ button, as a slow frame delivers a tap: press and release in the SAME
/// frame. Two separate faults made this do nothing while a hold worked - the
/// press was never seen as "down on" the button, and the opening click then
/// counted as a click outside the palette and closed it again.
#[test]
fn a_tap_on_the_command_button_opens_the_palette_and_it_stays_open() {
    let (mut st, _) = stopped_shell();
    let ctx = egui::Context::default();
    let c = uikit_egui::apply_palette(&ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::Night));
    let mut view = stoat_egui::View::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 760.0));
    let mut frame = |t: f64, events: Vec<egui::Event>, st: &mut stoat_egui::shell::ShellState, view: &mut stoat_egui::View| {
        let input = egui::RawInput { screen_rect: Some(screen), time: Some(t), events, ..Default::default() };
        #[allow(deprecated)]
        let out = ctx.run(input, |ctx| {
            #[allow(deprecated)]
            egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(10.0)).show(ctx, |ui| stoat_egui::draw(ui, st, view, &c));
        });
        uikit_egui::layout_probe::texts(&out.shapes)
    };
    let mut texts = Vec::new();
    for i in 0..3 {
        texts = frame(i as f64 * 0.016, vec![], &mut st, &mut view);
    }
    let button = texts.iter().find(|t| t.text == "⌘" || t.text == "cmd").expect("the command button was drawn");
    let pos = button.rect.center();
    let press = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };

    assert!(!view.palette.is_open());
    frame(1.0, vec![egui::Event::PointerMoved(pos)], &mut st, &mut view);
    frame(1.1, vec![press(true), press(false)], &mut st, &mut view);
    assert!(view.palette.is_open(), "the tap opened it");
    let texts = frame(1.2, vec![], &mut st, &mut view);
    frame(1.3, vec![], &mut st, &mut view);
    assert!(view.palette.is_open(), "and the click that opened it did not also close it");
    assert!(texts.iter().any(|t| t.text == "step over"), "the rows are on screen");

    // A click well outside it does close it.
    let away = egui::pos2(180.0, 120.0);
    let press_away = |pressed| egui::Event::PointerButton { pos: away, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame(2.0, vec![egui::Event::PointerMoved(away)], &mut st, &mut view);
    frame(2.1, vec![press_away(true), press_away(false)], &mut st, &mut view);
    frame(2.2, vec![], &mut st, &mut view);
    assert!(!view.palette.is_open(), "click-outside still dismisses");
}
