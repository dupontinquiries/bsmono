//! frame_bench — how much CPU one keyboard frame costs, headless, per board.
//!
//! Run:  cargo run --release --no-default-features --example frame_bench
//!
//! Drives `keyboard_ui` through a bare `egui::Context` (no window, no GPU, no eframe) for
//! each board the sidebar offers, idle and with a pointer wandering over the keys, and prints
//! µs per frame. The point is a NUMBER to put beside a change rather than a feeling: a phone
//! runs this loop on every touch-move, and a keyboard that costs 3 ms a frame at 60 Hz is a
//! fifth of a core doing nothing. Tessellation is included (it is what egui does after the UI
//! closure); rendering is not, because that is the GPU's and the shell's.
//!
//! The numbers move with the machine; compare within one run, before and after.

use std::time::Instant;

use rusty_board_egui::real_layout::{to_render, RealBoard, Shift};
use uikit_egui::Adaptive;
use rusty_board_egui::{keyboard_ui, Layout};

fn frame(ctx: &egui::Context, layout: &Layout, adaptive: &Adaptive, palette: &str, events: Vec<egui::Event>, t: f64) -> usize {
    let raw = egui::RawInput {
        events,
        time: Some(t),
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 340.0))),
        ..Default::default()
    };
    let out = ctx.run_ui(raw, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            keyboard_ui(ui, layout, adaptive, palette);
        });
    });
    // Tessellate like a real frame would; return the primitive count as a sanity signal.
    ctx.tessellate(out.shapes, out.pixels_per_point).len()
}

fn bench(name: &str, layout: &Layout, palette: &str) {
    let ctx = egui::Context::default();
    let adaptive = Adaptive { min_touch_target: 52.0, gutter: 6.0, ..Adaptive::default() };
    let warm = 20;
    let n = 300;
    let mut t = 0.0;
    for _ in 0..warm {
        t += 1.0 / 60.0;
        frame(&ctx, layout, &adaptive, palette, vec![], t);
    }
    // idle
    let start = Instant::now();
    let mut prims = 0;
    for _ in 0..n {
        t += 1.0 / 60.0;
        prims = frame(&ctx, layout, &adaptive, palette, vec![], t);
    }
    let idle_us = start.elapsed().as_micros() as f64 / n as f64;
    // a pointer sweeping across the board, pressed on every fourth frame — the touch-move case
    let start = Instant::now();
    for i in 0..n {
        t += 1.0 / 60.0;
        let x = 40.0 + (i as f32 * 7.0) % 1000.0;
        let y = 40.0 + (i as f32 * 3.0) % 260.0;
        let pos = egui::pos2(x, y);
        let mut ev = vec![egui::Event::PointerMoved(pos)];
        if i % 4 == 0 {
            ev.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: i % 8 == 0, modifiers: Default::default() });
        }
        frame(&ctx, layout, &adaptive, palette, ev, t);
    }
    let active_us = start.elapsed().as_micros() as f64 / n as f64;
    println!("{name:<28} {palette:<8} idle {idle_us:7.0} µs/frame   pointer {active_us:7.0} µs/frame   ({prims} primitives)");
}

fn main() {
    let real = RealBoard::new();
    let qwerty = to_render(&real.files[0], Shift::Off);
    let grid = to_render(real.files.iter().find(|f| f.name == "cardinal_grid_en").unwrap(), Shift::Off);
    let steno = to_render(real.files.iter().find(|f| f.name == "stenotype").unwrap(), Shift::Off);

    // How much a layout PROJECTION costs, since the screen used to do it every frame.
    let start = Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(to_render(&real.files[0], Shift::Off));
    }
    println!("to_render(qwerty)            {:7.1} µs/call", start.elapsed().as_micros() as f64 / 1000.0);
    let start = Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(to_render(real.files.iter().find(|f| f.name == "cardinal_grid_en").unwrap(), Shift::Off));
    }
    println!("to_render(cardinal grid)     {:7.1} µs/call", start.elapsed().as_micros() as f64 / 1000.0);
    println!();

    for palette in ["system", "default"] {
        bench("qwerty (real)", &qwerty, palette);
        bench("cardinal8 (demo)", &Layout::cardinal8(), palette);
        bench("cardinal grid (gtk ref)", &grid, palette);
        bench("stenotype", &steno, palette);
    }
}
