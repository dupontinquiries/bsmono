//! eink-demo — the first thing to launch on a tablet.
//!
//! Everything on it exists to answer one question on the device:
//! * big buttons + counter: does a finger tap land and click?
//! * colour swatches: is the colour path (and fast-color) working?
//! * drawing pad: how does the fast waveform look while dragging, and does
//!   the settle refresh clean it up?
//! * touch readout: raw digitizer vs mapped pixel coordinates, to find the
//!   right `EINK_TOUCH=swap,invx,invy` if taps land in the wrong place.
//! * full refresh: the ghost-clearing escape hatch.
//!
//! `eink-demo --snapshot out.ppm` renders one frame headless and exits.

use eink_kit::egui::{self, Color32, Pos2, Sense, Stroke, Vec2};

#[derive(Default)]
struct Demo {
    taps: u32,
    toggle: bool,
    level: f32,
    strokes: Vec<Vec<Pos2>>,
    corner_hits: [bool; 4],
    /// Motion test: a colour ball bouncing in the pad at 25 fps requested.
    animate: bool,
    ball: (f32, f32, f32, f32),
}

impl eink_kit::EinkApp for Demo {
    fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let d = eink_kit::diagnostics(&ctx);
        egui::CentralPanel::default().show_inside(ui, |ui| {
            // Under AppLoad the top ~50pt is its drag-down gesture strip.
            ui.add_space(40.0);
            ui.heading("eink-kit demo");
            ui.label(format!(
                "backend {}  ·  profile {}  ·  frame {}  ·  raster {:.1} ms  ·  {} dirty tiles",
                d.backend, d.profile, d.frames, d.last_raster_ms, d.last_dirty_tiles
            ));
            ui.separator();

            ui.horizontal(|ui| {
                let big = |t: &str| egui::Button::new(egui::RichText::new(t).size(26.0)).min_size(Vec2::new(150.0, 70.0));
                if ui.add(big("+1")).clicked() {
                    self.taps += 1;
                }
                if ui.add(big("reset")).clicked() {
                    self.taps = 0;
                    self.strokes.clear();
                    self.corner_hits = [false; 4];
                }
                if ui.add(big("full refresh")).clicked() {
                    eink_kit::request_full_refresh(ui.ctx());
                }
                if ui.add(big("quit")).clicked() {
                    std::process::exit(0);
                }
            });
            ui.label(egui::RichText::new(format!("taps: {}", self.taps)).size(30.0));
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.toggle, egui::RichText::new("a checkbox").size(22.0));
                ui.checkbox(&mut self.animate, egui::RichText::new("motion test (mono while moving, colour when stopped)").size(22.0));
            });
            ui.add(egui::Slider::new(&mut self.level, 0.0..=100.0).text("slider"));

            ui.horizontal(|ui| {
                for (name, c) in [("red", Color32::from_rgb(210, 30, 30)), ("green", Color32::from_rgb(30, 160, 60)),
                                  ("blue", Color32::from_rgb(30, 70, 200)), ("yellow", Color32::from_rgb(235, 200, 20))] {
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(90.0, 50.0), Sense::click());
                    ui.painter().rect_filled(r, 6.0, c);
                    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(16.0), Color32::WHITE);
                    if resp.clicked() {
                        self.taps += 1;
                    }
                }
            });

            ui.label(format!("motion: {} tiles · panel shows motion every {:.0} ms", d.motion_tiles, d.motion_interval * 1000.0));
            ui.label(match (d.last_touch, d.last_raw) {
                (Some((x, y)), Some((rx, ry))) => format!("last touch: panel ({x:.0}, {y:.0}) px  ·  raw ({rx}, {ry})"),
                (Some((x, y)), None) => format!("last touch: panel ({x:.0}, {y:.0}) px  (pre-mapped by display server)"),
                _ => "touch the screen…".to_string(),
            });

            // Drawing pad with corner calibration targets.
            ui.label("draw with a finger · tap the four corner dots · wrong spot? set EINK_TOUCH");
            let size = ui.available_size() - Vec2::new(0.0, 8.0);
            let (rect, resp) = ui.allocate_exact_size(size, Sense::drag());
            let p = ui.painter_at(rect);
            p.rect_stroke(rect, 4.0, Stroke::new(2.0_f32, Color32::BLACK), egui::StrokeKind::Inside);
            let corners = [rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom()];
            for (i, c) in corners.iter().enumerate() {
                let c = *c + (rect.center() - *c).normalized() * 30.0;
                p.circle_filled(c, 14.0, if self.corner_hits[i] { Color32::from_rgb(30, 160, 60) } else { Color32::BLACK });
                if resp.drag_started() && resp.interact_pointer_pos().is_some_and(|q| q.distance(c) < 40.0) {
                    self.corner_hits[i] = true;
                }
            }
            if resp.drag_started() {
                self.strokes.push(Vec::new());
            }
            if let (true, Some(pos)) = (resp.dragged(), resp.interact_pointer_pos()) {
                if let Some(s) = self.strokes.last_mut() {
                    s.push(pos);
                }
            }
            for s in &self.strokes {
                p.add(egui::Shape::line(s.clone(), Stroke::new(4.0_f32, Color32::BLACK)));
            }
            if self.animate {
                let (x, y, vx, vy) = &mut self.ball;
                if *vx == 0.0 { *vx = 9.0; *vy = 6.0; *x = rect.center().x; *y = rect.center().y; }
                *x += *vx;
                *y += *vy;
                if *x < rect.left() + 40.0 || *x > rect.right() - 40.0 { *vx = -*vx; }
                if *y < rect.top() + 40.0 || *y > rect.bottom() - 40.0 { *vy = -*vy; }
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(40));
            }
            if self.ball.2 != 0.0 {
                p.circle_filled(egui::pos2(self.ball.0, self.ball.1), 36.0, Color32::from_rgb(30, 70, 200));
                p.circle_filled(egui::pos2(self.ball.0, self.ball.1), 18.0, Color32::from_rgb(235, 200, 20));
            }
        });
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        let out = args.get(i + 1).cloned().unwrap_or_else(|| "eink-demo.ppm".into());
        std::process::exit(match snapshot(&out) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("snapshot: {e}");
                1
            }
        });
    }
    if let Err(e) = eink_kit::run(Demo { level: 40.0, ..Default::default() }) {
        eprintln!("eink-demo: {e}");
        std::process::exit(1);
    }
}

/// One headless frame at the Paper Pro's size (or `EINK_MODEL`'s), as a PPM.
fn snapshot(out: &str) -> std::io::Result<()> {
    use eink_kit::present::{mem::MemPresent, Present};
    let cfg = eink_kit::config::Config::from_env().map_err(std::io::Error::other)?;
    let model = eink_kit::device::Model::parse(&std::env::var("EINK_MODEL").unwrap_or_else(|_| "rmpp".into()))
        .unwrap_or(eink_kit::device::Model::PaperPro);
    let (w, h) = model.size();
    let mut p = MemPresent::new(w, h, eink_kit::format::PixelFormat::Rgb565, true, model.color());
    let mut e = eink_kit::Engine::new(&cfg, p.caps(), "mem");
    let motion = std::env::var("EINK_SNAPSHOT_MOTION").as_deref() == Ok("1");
    let mut app = Demo { taps: 3, toggle: true, level: 40.0, animate: motion, ..Default::default() };
    app.strokes.push((0..40).map(|i| Pos2::new(120.0 + i as f32 * 6.0, 700.0 + (i as f32 * 0.4).sin() * 40.0)).collect());
    // Two frames: fonts are uploaded on the first. With the motion test on,
    // run a simulated second so the ball is classified and dithered.
    e.frame(std::time::Duration::ZERO, &mut app, &mut p)?;
    let mut r = e.frame(std::time::Duration::from_millis(50), &mut app, &mut p)?;
    if motion {
        for t in (100..1200).step_by(5) {
            let now = std::time::Duration::from_millis(t);
            if e.wants_frame(now) {
                r = e.frame(now, &mut app, &mut p)?;
            }
            e.tick(now, &mut p)?;
        }
    }
    eprintln!("snapshot: {w}x{h}, raster {:.1} ms, {} dirty tiles", r.raster.as_secs_f32() * 1000.0, r.dirty_tiles);
    p.write_ppm(std::path::Path::new(out))
}
