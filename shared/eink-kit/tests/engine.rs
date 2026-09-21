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
