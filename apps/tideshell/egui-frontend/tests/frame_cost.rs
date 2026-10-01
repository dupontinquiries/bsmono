//! What one frame of the shell costs on the CPU before any pixel is written: building the
//! egui frame (ShellView::ui) and tessellating it, at the FP5's logical size. On a software
//! rasterizer these ARE most of the frame; the vertex count is what the painter then fills.
//!
//! A measurement, not a pass/fail test: `cargo test --release --test frame_cost -- --ignored
//! --nocapture`. Run on a quiet machine; compare states against each other and runs against
//! runs, not against a threshold.

use egui::{Pos2, Rect, Vec2};
use tideshell_core::prefs::ShellPrefs;
use tideshell_core::shell::Shell;
use tideshell_egui::{FrameInfo, ShellView};

fn measure(name: &str, setup: impl Fn(&mut Shell)) {
    let mut sh = Shell::new(ShellPrefs::default(), (408.0, 900.0));
    setup(&mut sh);
    let mut view = ShellView::default();
    let pal = uikit_egui::palette_colors("night");
    let ctx = egui::Context::default();
    let (mut build, mut tess) = (Vec::new(), Vec::new());
    let (mut verts, mut prims) = (0usize, 0usize);
    let frames: usize = std::env::var("FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(220);
    for i in 0..frames {
        let info = FrameInfo { apps: &[], time: tideshell_core::clock::LocalTime { hour: 12, minute: 34, weekday: 3 }, battery: Some(0.7), charging: false, trackpad_note: None, t_secs: i as f32 / 60.0 };
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(408.0, 900.0))), time: Some(i as f64 / 60.0), ..Default::default() };
        let t0 = std::time::Instant::now();
        let out = ctx.run_ui(raw, |ui| { let _ = view.ui(ui, &mut sh, &pal, &info); });
        let t1 = std::time::Instant::now();
        let p = ctx.tessellate(out.shapes, out.pixels_per_point);
        let t2 = std::time::Instant::now();
        if i >= frames / 10 {
            // The first frames build fonts and caches; steady state is what the phone lives in.
            build.push((t1 - t0).as_secs_f64() * 1e3);
            tess.push((t2 - t1).as_secs_f64() * 1e3);
            prims = p.len();
            verts = p.iter().map(|c| match &c.primitive { egui::epaint::Primitive::Mesh(m) => m.vertices.len(), _ => 0 }).sum();
        }
    }
    let q = |v: &mut Vec<f64>, f: f64| { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[((v.len() - 1) as f64 * f) as usize] };
    println!("{name:<12} build p50 {:6.3} ms p95 {:6.3}   tessellate p50 {:6.3} p95 {:6.3}   {prims:4} prims {verts:6} verts",
             q(&mut build, 0.5), q(&mut build, 0.95), q(&mut tess, 0.5), q(&mut tess, 0.95));
}

#[test]
#[ignore]
fn frame_cost_by_state() {
    measure("home", |_| {});
    measure("locked", |s| s.lock_now());
    measure("palette", |s| s.open_palette());
    measure("switcher", |s| s.open_switcher());
}
