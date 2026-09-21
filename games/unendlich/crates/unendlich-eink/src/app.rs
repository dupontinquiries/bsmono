//! The tablet app: a view of the world you steer by touch.
//!
//! Progressive resolution fits e-ink: while you drag or hold a button, a
//! low-resolution frame (cheap, and drawn by eink-kit's fast mono motion
//! lane anyway); once you stop, one high-resolution frame, which eink-kit
//! settles in colour. While the height cache is still filling, the view is
//! re-rendered as it sharpens. The view rect is hinted as Video so eink-kit
//! treats it as motion from the first change.

use crate::heights::{HeightCache, World};
use crate::view::{render, Cam, Stats};
use eink_kit::egui::{self, Color32, ColorImage, RichText, Sense, TextureHandle, TextureOptions, Vec2};
use std::sync::Arc;
use std::time::{Duration, Instant};

const LO: (usize, usize) = (135, 180);
const HI: (usize, usize) = (405, 540);
const QUIET: Duration = Duration::from_millis(350);

pub struct Unendlich {
    pub cam: Cam,
    pub seed: u32,
    sea: f32,
    cache: Arc<HeightCache>,
    tex: Option<TextureHandle>,
    /// (camera, cache fill count, hi-res?) of the image on screen.
    shown: Option<(Cam, u64, bool)>,
    last_input: Instant,
    pub last: Stats,
    pub last_ms: f32,
}

impl Unendlich {
    pub fn new(seed: u32) -> Unendlich {
        let gen = unendlich_engine::world::cpu_generator::WorldGenerator::new(seed);
        let sea = gen.sea_level_at(0, 0) as f32;
        let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).saturating_sub(1).max(1);
        let cache = Arc::new(HeightCache::new(Arc::new(World(gen)), workers));
        let mut cam = Cam::default();
        cam.y = sea + 45.0;
        Unendlich { cam, seed, sea, cache, tex: None, shown: None, last_input: Instant::now() - QUIET * 2, last: Stats::default(), last_ms: 0.0 }
    }

    pub fn cache(&self) -> &HeightCache {
        &self.cache
    }

    fn moved(&mut self) {
        self.last_input = Instant::now();
        self.cache.clear_queue();
    }

    fn step(&mut self, forward: f32, strafe: f32) {
        let (c, s) = (self.cam.yaw.cos(), self.cam.yaw.sin());
        self.cam.x += c * forward - s * strafe;
        self.cam.z += s * forward + c * strafe;
        self.moved();
    }

    /// Render if the camera, the cache, or the wanted resolution changed.
    fn refresh(&mut self, ctx: &egui::Context) {
        let interacting = self.last_input.elapsed() < QUIET;
        let hi = !interacting;
        let filled = self.cache.filled();
        let want = (self.cam, filled, hi);
        if self.shown == Some(want) {
            return;
        }
        // Cache fills alone re-render at most every 400 ms (e-ink pace).
        if let Some((cam, f, h)) = self.shown {
            if cam == self.cam && h == hi && f != filled && self.last_input.elapsed() < Duration::from_millis(400) && interacting {
                return;
            }
        }
        let (w, h) = if hi { HI } else { LO };
        let mut px = vec![0u8; w * h * 4];
        let t = Instant::now();
        let cache = self.cache.clone();
        self.last = render(&self.cam, w, h, self.sea, &|l, cx, cz| cache.lookup(l, cx, cz), &mut px);
        self.last_ms = t.elapsed().as_secs_f32() * 1000.0;
        let img = ColorImage::from_rgba_unmultiplied([w, h], &px);
        // NEAREST: crisp block pixels on e-ink, no blur from upscaling.
        match &mut self.tex {
            Some(t) => t.set(img, TextureOptions::NEAREST),
            None => self.tex = Some(ctx.load_texture("unendlich-view", img, TextureOptions::NEAREST)),
        }
        self.shown = Some(want);
        self.cache.evict_far(self.cam.x, self.cam.z);
    }
}

impl eink_kit::EinkApp for Unendlich {
    fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.add_space(36.0); // AppLoad's drag-down strip
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("unendlich").strong().size(22.0));
                ui.label(format!("seed {}  ·  x {:.0} z {:.0} y {:.0}", self.seed, self.cam.x, self.cam.z, self.cam.y));
            });
            ui.label(RichText::new(format!(
                "{:.0} ms/frame  ·  {} cached  ·  {} pending  ·  {} of {} samples unknown",
                self.last_ms, self.cache.len(), self.cache.pending(), self.last.misses, self.last.samples)).small());
            let big = |t: &str| egui::Button::new(RichText::new(t).size(24.0)).min_size(Vec2::new(86.0, 58.0));
            ui.horizontal(|ui| {
                // Words, not ▲/▼: those glyphs are not in egui's default fonts
                // and rendered as boxes on the panel.
                if ui.add(big("◀")).clicked() { self.step(0.0, -16.0); }
                if ui.add(big("fwd")).clicked() { self.step(24.0, 0.0); }
                if ui.add(big("back")).clicked() { self.step(-24.0, 0.0); }
                if ui.add(big("▶")).clicked() { self.step(0.0, 16.0); }
                if ui.add(big("up")).clicked() { self.cam.y += 10.0; self.moved(); }
                if ui.add(big("down")).clicked() { self.cam.y = (self.cam.y - 10.0).max(self.sea + 2.0); self.moved(); }
                if ui.add(big("quit")).clicked() { std::process::exit(0); }
            });
            ui.label(RichText::new("drag on the view to turn and look up/down").small());

            self.refresh(&ctx);
            let size = ui.available_size();
            let (rect, resp) = ui.allocate_exact_size(size, Sense::drag());
            eink_kit::hint(&ctx, rect, eink_kit::Hint::Video);
            if resp.dragged() {
                let d = resp.drag_delta();
                self.cam.yaw += d.x / rect.width().max(1.0) * self.cam.fov * 1.5;
                self.cam.horizon = (self.cam.horizon + d.y / rect.height().max(1.0)).clamp(0.05, 0.95);
                self.moved();
            }
            if let Some(t) = &self.tex {
                // fit the image's aspect inside the rect
                let [iw, ih] = t.size();
                let s = (rect.width() / iw as f32).min(rect.height() / ih as f32);
                let r = egui::Rect::from_center_size(rect.center(), Vec2::new(iw as f32 * s, ih as f32 * s));
                ui.painter().image(t.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            }
        });
        // Keep going while the view can still sharpen or should go hi-res.
        if self.cache.pending() > 0 {
            ctx.request_repaint_after(Duration::from_millis(300));
        } else if self.last_input.elapsed() < QUIET {
            ctx.request_repaint_after(QUIET);
        }
    }
}
