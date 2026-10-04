//! Motion management end to end: fake clock, real egui, in-memory panel.

use eink_kit::config::Config;
use eink_kit::format::{is_chromatic, PixelFormat};
use eink_kit::policy::{Update, Waveform};
use eink_kit::present::mem::MemPresent;
use eink_kit::present::Present;
use eink_kit::touch::{Phase, TouchEvent};
use eink_kit::{EinkApp, Engine, Hint};
use egui::{Color32, Rect, Vec2};
use std::time::Duration;

fn cfg() -> Config {
    let mut c = Config::from_env().unwrap();
    c.profile = None;
    c.scale = 2.0;
    c.threads = 2;
    c.motion_hz = 6.0;
    c.motion_color = false;
    c.dither = true;
    c
}
fn ms(v: u64) -> Duration { Duration::from_millis(v) }

/// Run the engine like `run` does, on a 5 ms fake clock. Returns frames rendered and updates.
fn drive(e: &mut Engine, app: &mut dyn EinkApp, p: &mut MemPresent, from: u64, to: u64) -> (u32, Vec<Update>) {
    let (mut frames, mut ups) = (0, vec![]);
    let mut t = from;
    while t < to {
        if e.wants_frame(ms(t)) {
            frames += 1;
            ups.extend(e.frame(ms(t), app, p).unwrap().updates);
        }
        ups.extend(e.tick(ms(t), p).unwrap());
        t += 5;
    }
    (frames, ups)
}

struct Video { playing: bool, t: u32, rect: Rect, hinted: bool }
impl EinkApp for Video {
    fn ui(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.label("player");
            let (r, _) = ui.allocate_exact_size(Vec2::new(256.0, 160.0), egui::Sense::hover());
            self.rect = r;
            if self.hinted { eink_kit::hint(ui.ctx(), r, Hint::Video); }
            for i in 0..8 {
                let x = r.min.x + i as f32 * 32.0;
                let k = ((i as u32 * 30 + self.t * 17) % 200) as u8;
                ui.painter().rect_filled(Rect::from_min_size(egui::pos2(x, r.min.y), Vec2::new(32.0, 160.0)), 0.0,
                    // Brightness must vary: a pure hue shift is (correctly)
                    // invisible after mono dithering, so it makes no damage.
                    Color32::from_rgb(k, k / 2 + 20, 255 - k));
            }
            if self.playing {
                self.t += 1;
                ui.ctx().request_repaint_after(ms(40)); // 25 fps
            }
        });
    }
}

fn region_pixels(p: &MemPresent, r: Rect) -> Vec<[u8; 4]> {
    let (x0, y0, x1, y1) = ((r.min.x * 2.0) as u32 + 4, (r.min.y * 2.0) as u32 + 4, (r.max.x * 2.0) as u32 - 4, (r.max.y * 2.0) as u32 - 4);
    (y0..y1).step_by(7).flat_map(|y| (x0..x1).step_by(5).map(move |x| (x, y))).map(|(x, y)| p.pixel(x, y)).collect()
}

#[test]
fn video_plays_mono_dithered_at_the_cap_and_settles_to_colour_on_pause() {
    let mut p = MemPresent::new(702, 936, PixelFormat::Rgb565, true, true);
    let mut e = Engine::new(&cfg(), p.caps(), "mem");
    let mut v = Video { playing: true, t: 0, rect: Rect::NOTHING, hinted: true };
    drive(&mut e, &mut v, &mut p, 0, 1000); // warm-up
    let (frames, ups) = drive(&mut e, &mut v, &mut p, 1000, 3000);

    // Paced to the panel: ~6 Hz, not the 25 fps the app asked for.
    assert!(frames <= 15, "{frames} frames rendered in 2 s; decoder work should be paced to ~6 Hz");
    assert!(frames >= 9, "{frames} frames: video stalled");
    let motion: Vec<_> = ups.iter().filter(|u| u.reason.starts_with("motion")).collect();
    assert!(motion.len() >= 9 && motion.len() <= 14, "{} motion updates", motion.len());
    assert!(motion.iter().all(|u| u.waveform == Waveform::Fast && !u.color), "{motion:?}");
    assert!(ups.iter().all(|u| u.reason.starts_with("motion")), "nothing else refreshes during playback: {ups:?}");
    // Dithered: only black and white inside the picture.
    assert!(region_pixels(&p, v.rect).iter().all(|px| px[0] == px[1] && (px[0] < 8 || px[0] > 247)), "not dithered");

    // Pause.
    v.playing = false;
    let (_, ups) = drive(&mut e, &mut v, &mut p, 3000, 5000);
    let settle: Vec<_> = ups.iter().filter(|u| u.waveform == Waveform::Quality).collect();
    assert_eq!(settle.len(), 1, "one quality settle after pause: {ups:?}");
    assert!(settle[0].color, "the paused frame comes back in colour");
    let colourful = region_pixels(&p, v.rect).iter().filter(|px| is_chromatic(px[0], px[1], px[2])).count();
    assert!(colourful > 20, "paused picture should be colour again, {colourful} chromatic samples");
    // And then silence.
    let (frames, ups) = drive(&mut e, &mut v, &mut p, 5000, 7000);
    assert!(ups.is_empty() && frames == 0, "idle after settle: {frames} frames, {ups:?}");
}

#[test]
fn an_unhinted_playhead_is_detected_as_motion() {
    struct Timeline { x: f32, running: bool }
    impl EinkApp for Timeline {
        fn ui(&mut self, ui: &mut egui::Ui) {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                let r = ui.max_rect();
                for i in 0..6 {
                    let c = [Color32::RED, Color32::BLUE, Color32::GREEN][i % 3];
                    ui.painter().rect_filled(Rect::from_min_size(egui::pos2(20.0 + i as f32 * 50.0, 100.0), Vec2::new(40.0, 30.0)), 2.0, c);
                }
                ui.painter().vline(self.x, r.y_range(), egui::Stroke::new(2.0_f32, Color32::BLACK));
                if self.running {
                    self.x += 3.0;
                    ui.ctx().request_repaint_after(ms(33));
                }
            });
        }
    }
    let mut p = MemPresent::new(702, 936, PixelFormat::Rgb565, true, true);
    let mut e = Engine::new(&cfg(), p.caps(), "mem");
    let mut app = Timeline { x: 30.0, running: true };
    drive(&mut e, &mut app, &mut p, 0, 800);
    let (frames, ups) = drive(&mut e, &mut app, &mut p, 800, 2800);
    let motion = ups.iter().filter(|u| u.reason.starts_with("motion")).count();
    assert!(motion >= 8, "playhead not classified as motion: {ups:?}");
    assert!(frames <= 15, "{frames} frames: not paced");
    app.running = false;
    let (_, ups) = drive(&mut e, &mut app, &mut p, 2800, 4500);
    assert!(ups.iter().any(|u| u.waveform == Waveform::Quality), "no settle after stop: {ups:?}");
}

#[test]
fn a_finger_drag_is_interaction_not_motion() {
    struct Slider { v: f32, rect: Rect }
    impl EinkApp for Slider {
        fn ui(&mut self, ui: &mut egui::Ui) {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                ui.spacing_mut().slider_width = 300.0;
                self.rect = ui.add(egui::Slider::new(&mut self.v, 0.0..=100.0)).rect;
            });
        }
    }
    let mut p = MemPresent::new(702, 936, PixelFormat::Rgb565, true, true);
    let mut e = Engine::new(&cfg(), p.caps(), "mem");
    let mut app = Slider { v: 0.0, rect: Rect::NOTHING };
    drive(&mut e, &mut app, &mut p, 0, 500);
    let y = app.rect.center().y * 2.0;
    let x0 = app.rect.min.x * 2.0 + 10.0;
    e.push_touch(ms(500), &TouchEvent { id: 0, phase: Phase::Start, x: x0, y });
    let mut frames = 0;
    let mut ups = vec![];
    for i in 1..=40u64 {
        let t = 500 + i * 20; // 50 Hz finger
        e.push_touch(ms(t), &TouchEvent { id: 0, phase: Phase::Move, x: x0 + i as f32 * 12.0, y });
        let (f, u) = drive(&mut e, &mut app, &mut p, t, t + 20);
        frames += f;
        ups.extend(u);
    }
    assert!(app.v > 50.0, "slider did not follow the finger: {}", app.v);
    assert!(!ups.iter().any(|u| u.reason.starts_with("motion")), "drag was throttled as motion");
    assert!(frames >= 35, "{frames} frames: interaction must not be paced");
    assert!(ups.iter().all(|u| u.waveform == Waveform::Fast));
}

/// Drive the engine exactly as `run()` does: render when a frame is wanted,
/// tick, then SLEEP until `next_wake()` (or forever: stop). The 5 ms ticker
/// above hid a real bug: with nothing scheduled, the loop slept forever and
/// the end of motion was never noticed, leaving the last dithered frame on
/// the panel indefinitely.
fn drive_like_run(e: &mut Engine, app: &mut dyn EinkApp, p: &mut MemPresent, from: u64, to: u64) -> (u32, Vec<Update>) {
    let (mut frames, mut ups) = (0, vec![]);
    let mut t = from;
    while t < to {
        if e.wants_frame(ms(t)) {
            frames += 1;
            ups.extend(e.frame(ms(t), app, p).unwrap().updates);
        }
        ups.extend(e.tick(ms(t), p).unwrap());
        if e.wants_frame(ms(t)) {
            continue;
        }
        match e.next_wake() {
            Some(w) => t = (w.as_millis() as u64).max(t + 1),
            None => break, // run() would block on input forever here
        }
    }
    (frames, ups)
}

#[test]
fn paused_video_settles_even_when_nothing_else_wakes_the_loop() {
    let mut p = MemPresent::new(702, 936, PixelFormat::Rgb565, true, true);
    let mut e = Engine::new(&cfg(), p.caps(), "mem");
    let mut v = Video { playing: true, t: 0, rect: Rect::NOTHING, hinted: true };
    drive_like_run(&mut e, &mut v, &mut p, 0, 2000);
    v.playing = false;
    let (_, ups) = drive_like_run(&mut e, &mut v, &mut p, 2000, 6000);
    assert!(ups.iter().any(|u| u.waveform == Waveform::Quality && u.color),
        "the paused frame never settled back to colour: {ups:?}");
    let colourful = region_pixels(&p, v.rect).iter().filter(|px| is_chromatic(px[0], px[1], px[2])).count();
    assert!(colourful > 20, "still dithered after pause ({colourful} chromatic samples)");
}

#[test]
fn a_few_startup_relayouts_are_not_motion() {
    // egui settles layout over its first frames: three changes 120 ms apart
    // must not be classified as motion (they were, dithering the whole top of
    // tomescope's library screen).
    struct Settling { n: u32 }
    impl EinkApp for Settling {
        fn ui(&mut self, ui: &mut egui::Ui) {
            egui::CentralPanel::default().show_inside(ui, |ui| { ui.heading(format!("pass {}", self.n.min(2))); });
            if self.n < 2 { self.n += 1; ui.ctx().request_repaint(); }
        }
    }
    let mut p = MemPresent::new(702, 936, PixelFormat::Rgb565, true, true);
    let mut e = Engine::new(&cfg(), p.caps(), "mem");
    let (_, ups) = drive_like_run(&mut e, &mut Settling { n: 0 }, &mut p, 0, 3000);
    assert!(!ups.iter().any(|u| u.reason.starts_with("motion")), "startup churn treated as motion: {ups:?}");
}
