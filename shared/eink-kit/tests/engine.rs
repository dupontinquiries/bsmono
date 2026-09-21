//! End to end through the engine with real egui widgets, the in-memory
//! backend, and a fake clock.

use eink_kit::config::Config;
use eink_kit::format::PixelFormat;
use eink_kit::policy::Waveform;
use eink_kit::present::mem::MemPresent;
use eink_kit::present::Present;
use eink_kit::touch::{Phase, TouchEvent};
use eink_kit::Engine;
use std::time::Duration;

fn cfg() -> Config {
    let mut c = Config::from_env().unwrap();
    c.profile = None;
    c.scale = 2.0;
    c.threads = 4;
    c.trace = false;
    c
}

fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}

struct Counter {
    taps: u32,
    button: egui::Rect,
}

impl eink_kit::EinkApp for Counter {
    fn ui(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.heading("Counter");
            let b = ui.add_sized([200.0, 80.0], egui::Button::new("Tap"));
            self.button = b.rect;
            if b.clicked() {
                self.taps += 1;
            }
            ui.label(format!("taps: {}", self.taps));
            ui.colored_label(egui::Color32::RED, "red text");
        });
    }
}

/// Button centre in panel pixels, from egui's own layout (points × scale).
fn button_centre(app: &Counter) -> (f32, f32) {
    let c = app.button.center();
    (c.x * 2.0, c.y * 2.0)
}

#[test]
fn a_finger_tap_clicks_a_real_button_and_the_panel_settles() {
    let mut p = MemPresent::new(702, 936, PixelFormat::Rgb565, true, true);
    let mut e = Engine::new(&cfg(), p.caps(), "mem");
    let mut app = Counter { taps: 0, button: egui::Rect::NOTHING };

    // First frame paints everything.
    let r = e.frame(ms(0), &mut app, &mut p).unwrap();
    assert!(!r.updates.is_empty(), "first frame must draw");
    assert!(r.dirty_tiles > 0);

    // Text actually rendered: some dark pixels in the heading area.
    let dark = (10..300).flat_map(|x| (10..60).map(move |y| (x, y))).filter(|&(x, y)| p.pixel(x, y)[0] < 100).count();
    assert!(dark > 50, "heading text should leave dark pixels, found {dark}");

    // An idle app requests no further frames (no animation busy-loop).
    e.frame(ms(200), &mut app, &mut p).unwrap();
    let r = e.frame(ms(400), &mut app, &mut p).unwrap();
    assert!(r.updates.is_empty(), "idle frame must not refresh the panel: {:?}", r.updates);

    // Tap.
    let (x, y) = button_centre(&app);
    assert!(app.button.width() > 100.0, "button laid out: {:?}", app.button);
    e.push_touch(ms(1000), &TouchEvent { id: 0, phase: Phase::Start, x, y });
    let down = e.frame(ms(1000), &mut app, &mut p).unwrap();
    e.push_touch(ms(1080), &TouchEvent { id: 0, phase: Phase::End, x, y });
    let up = e.frame(ms(1080), &mut app, &mut p).unwrap();
    // egui reports the click on release; the label updates next frame.
    e.frame(ms(1100), &mut app, &mut p).unwrap();
    assert_eq!(app.taps, 1, "button was not clicked");

    // Interaction was drawn fast (press highlight and/or label change).
    let fast: Vec<_> = down.updates.iter().chain(up.updates.iter()).filter(|u| u.waveform == Waveform::Fast).collect();
    assert!(!fast.is_empty(), "press feedback should use the fast waveform");

    // After the settle window, a quality clean-up of the touched region.
    let settle = e.tick(ms(1100 + 500), &mut p).unwrap();
    assert_eq!(settle.len(), 1, "exactly one settle refresh");
    assert_eq!(settle[0].waveform, Waveform::Quality);
    assert!(e.tick(ms(5000), &mut p).unwrap().is_empty());
}

#[test]
fn colour_content_is_flagged_colour_and_grey_is_not() {
    let mut p = MemPresent::new(702, 936, PixelFormat::Rgb888, true, true);
    let mut e = Engine::new(&cfg(), p.caps(), "mem");
    let mut grey = |ui: &mut egui::Ui| {
        egui::CentralPanel::default().show_inside(ui, |ui| { ui.label("just grey text"); });
    };
    let r = e.frame(ms(0), &mut grey, &mut p).unwrap();
    assert!(r.updates.iter().all(|u| !u.color), "grey UI flagged colour: {:?}", r.updates);

    let mut red = |ui: &mut egui::Ui| {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.label("just grey text");
            let (rect, _) = ui.allocate_exact_size(egui::vec2(60.0, 60.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 0.0, egui::Color32::from_rgb(200, 30, 30));
        });
    };
    let r = e.frame(ms(500), &mut red, &mut p).unwrap();
    assert!(r.updates.iter().any(|u| u.color), "red square not flagged: {:?}", r.updates);
}

#[test]
fn a_full_refresh_request_from_the_app_reaches_the_panel() {
    let mut p = MemPresent::new(400, 400, PixelFormat::Gray8, true, false);
    let mut e = Engine::new(&cfg(), p.caps(), "mem");
    let mut asked = false;
    let mut app = |ui: &mut egui::Ui| {
        if !asked {
            eink_kit::request_full_refresh(ui.ctx());
            asked = true;
        }
        ui.label("x");
    };
    e.frame(ms(0), &mut app, &mut p).unwrap();
    let u = e.tick(ms(1), &mut p).unwrap();
    assert_eq!(u.len(), 1);
    assert_eq!(u[0].waveform, Waveform::Full);
}

/// As `run()` does: render when wanted, tick, then sleep to next_wake.
fn drive_like_run(e: &mut Engine, app: &mut Counter, p: &mut MemPresent, from: u64, to: u64) {
    let mut t = from;
    while t < to {
        if e.wants_frame(ms(t)) { e.frame(ms(t), app, p).unwrap(); }
        e.tick(ms(t), p).unwrap();
        if e.wants_frame(ms(t)) { continue; }
        match e.next_wake() { Some(w) => t = (w.as_millis() as u64).max(t + 1), None => break }
    }
}

/// The Paper Pro incident: a hand resting on the screen for a long time
/// (AppLoad re-sends stationary points as PRESS and never sends a release)
/// left every button dead. Both cases must recover, and the palm must never
/// click what it rested on.
#[test]
fn a_resting_palm_never_blocks_the_ui_or_clicks_what_it_covers() {
    let mut p = MemPresent::new(702, 936, PixelFormat::Rgb565, true, true);
    let mut e = Engine::new(&cfg(), p.caps(), "mem");
    let mut app = Counter { taps: 0, button: egui::Rect::NOTHING };
    drive_like_run(&mut e, &mut app, &mut p, 0, 500);
    let (bx, by) = button_centre(&app);

    // 1. palm lands ON the button and rests; AppLoad re-sends it as PRESS
    e.push_touch(ms(1000), &TouchEvent { id: 50, phase: Phase::Start, x: bx, y: by });
    for i in 1..6 {
        e.push_touch(ms(1000 + i * 100), &TouchEvent { id: 50, phase: Phase::Start, x: bx, y: by });
        drive_like_run(&mut e, &mut app, &mut p, 1000 + i * 100, 1000 + i * 100 + 50);
    }
    // ...a finger taps the button while the palm is still down
    e.push_touch(ms(1700), &TouchEvent { id: 51, phase: Phase::Start, x: bx + 4.0, y: by });
    drive_like_run(&mut e, &mut app, &mut p, 1700, 1760);
    e.push_touch(ms(1760), &TouchEvent { id: 51, phase: Phase::End, x: bx + 4.0, y: by });
    drive_like_run(&mut e, &mut app, &mut p, 1760, 1900);
    assert_eq!(app.taps, 1, "the finger's tap counts; the palm did not click on its way out");

    // 2. the palm is never released: the loop must wake by itself and let it go
    drive_like_run(&mut e, &mut app, &mut p, 1900, 9000);
    assert!(!e.touch_debug_any_down(), "palm still held after 7 s of silence");
    // ...and a later tap works
    e.push_touch(ms(10_000), &TouchEvent { id: 52, phase: Phase::Start, x: bx, y: by });
    drive_like_run(&mut e, &mut app, &mut p, 10_000, 10_060);
    e.push_touch(ms(10_060), &TouchEvent { id: 52, phase: Phase::End, x: bx, y: by });
    drive_like_run(&mut e, &mut app, &mut p, 10_060, 10_300);
    assert_eq!(app.taps, 2, "UI dead after a resting palm");
}
