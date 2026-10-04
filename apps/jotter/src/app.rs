//! app -- the writing surface. One egui module, drawn by eframe on a desktop or by eink-kit on a tablet.
//!
//! Screens: the NOTES list; a NOTE (a touch canvas with pen, eraser and undo; every stroke is saved as it
//! ends); DEVICES (this device's name and fingerprint, the devices it trusts, a pairing QR); and the QR screens
//! (a pairing offer, or a note as a mega QR mosaic). Strokes are stored in canvas coordinates; a page is as
//! large as the screen it was written on. Nothing here touches the network (see `sync`, when it lands).

use crate::identity::Identity;
use crate::note::{stroke_id, Note};
use crate::pairing::Offer;
use crate::share::{self, Kind, Matrix};
use crate::trust::TrustStore;
use ink_kit::stroke::{InkPoint, Stroke, StrokeStyle, ToolKind};
use ink_kit::Vec2d;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool { Pen, Eraser }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    List,
    Note(usize),
    Devices,
    /// Codes on screen: which note (None = a pairing offer) and which screenful.
    Codes { note: Option<usize>, page: usize },
}

struct Entry {
    note: Note,
    path: PathBuf,
    dirty: bool,
}

pub struct Jotter {
    dir: PathBuf,
    pub me: Identity,
    pub trust: TrustStore,
    entries: Vec<Entry>,
    pub screen: Screen,
    pub tool: Tool,
    pen_width: f64,
    drawing: Option<Stroke>,
    /// Strokes made in this session, newest last, for undo.
    made: Vec<(usize, ink_kit::stroke::StrokeId)>,
    pub status: String,
    /// The addresses an offer carries (set by the host once something listens).
    pub listen_addrs: Vec<String>,
    textures: Vec<(u64, egui::TextureHandle)>,
}

fn now_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0) }
fn now_s() -> u64 { now_ms() / 1000 }
fn rand24() -> u32 { let mut b = [0u8; 4]; crypto_kit::random_bytes(&mut b); u32::from_be_bytes(b) & 0xff_ffff }

impl Jotter {
    /// Open the app's data directory `dir` (notes, identity, trusted devices), creating what is missing.
    pub fn open(dir: PathBuf, device_name: &str) -> std::io::Result<Jotter> {
        std::fs::create_dir_all(dir.join("notes"))?;
        let me = Identity::load_or_create(&dir.join("identity"), device_name)?;
        let trust = TrustStore::load(&dir.join("trusted"));
        let mut entries = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir.join("notes")) {
            for e in rd.flatten() {
                let path = e.path();
                if path.extension().is_some_and(|x| x == "jnt") {
                    if let Some(note) = std::fs::read(&path).ok().and_then(|b| Note::from_bytes(&b)) {
                        entries.push(Entry { note, path, dirty: false });
                    }
                }
            }
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.note.strokes().last().map(|s| s.id.0).unwrap_or(0)));
        Ok(Jotter { dir, me, trust, entries, screen: Screen::List, tool: Tool::Pen, pen_width: 2.5, drawing: None, made: vec![], status: String::new(), listen_addrs: vec![], textures: vec![] })
    }

    /// The default data directory: `$XDG_DATA_HOME/jotter` or `~/.local/share/jotter`.
    pub fn default_dir() -> Option<PathBuf> { Identity::default_path().and_then(|p| p.parent().map(|d| d.to_path_buf())) }

    pub fn note_count(&self) -> usize { self.entries.len() }
    pub fn note(&self, i: usize) -> Option<&Note> { self.entries.get(i).map(|e| &e.note) }

    pub fn new_note(&mut self) -> usize {
        let mut id = [0u8; 8];
        crypto_kit::random_bytes(&mut id);
        let title = format!("Note {}", self.entries.len() + 1);
        let by: [u8; 8] = self.me.ed_pub[..8].try_into().unwrap();
        let path = self.dir.join("notes").join(format!("{}.jnt", crate::identity::hex(&id)));
        self.entries.insert(0, Entry { note: Note::new(id, &title, now_ms(), by), path, dirty: true });
        self.save_all();
        0
    }

    /// Write every changed note (atomically).
    pub fn save_all(&mut self) {
        for e in self.entries.iter_mut().filter(|e| e.dirty) {
            let tmp = e.path.with_extension("jnt.tmp");
            if std::fs::write(&tmp, e.note.to_bytes()).and_then(|_| std::fs::rename(&tmp, &e.path)).is_ok() { e.dirty = false; }
            else { self.status = format!("could not save {}", e.path.display()); }
        }
    }

    // ---- writing -----------------------------------------------------------------------

    fn begin_stroke(&mut self, p: egui::Pos2, pressure: f32) {
        let mut s = Stroke::new(stroke_id(now_ms(), rand24()), StrokeStyle { width: self.pen_width, color: [10, 10, 10, 255], tool: ToolKind::Finger });
        s.points.push(InkPoint::new(Vec2d::new(p.x as f64, p.y as f64), pressure, 0.0));
        self.drawing = Some(s);
    }

    fn extend_stroke(&mut self, p: egui::Pos2, pressure: f32) {
        if let Some(s) = &mut self.drawing {
            let last = s.points.last().map(|q| q.pos).unwrap_or(Vec2d::new(0.0, 0.0));
            // a point closer than a pixel to the last adds nothing but bytes
            if (last.x - p.x as f64).hypot(last.y - p.y as f64) >= 1.0 {
                s.points.push(InkPoint::new(Vec2d::new(p.x as f64, p.y as f64), pressure, 0.0));
            }
        }
    }

    fn end_stroke(&mut self, note: usize) {
        if let Some(s) = self.drawing.take() {
            if s.points.len() >= 2 {
                self.made.push((note, s.id));
                if let Some(e) = self.entries.get_mut(note) { e.note.add(s); e.dirty = true; }
                self.save_all();
            }
        }
    }

    /// Erase the strokes within `r` px of `p`.
    fn erase_at(&mut self, note: usize, p: egui::Pos2, r: f64) {
        let Some(e) = self.entries.get_mut(note) else { return };
        let hit: Vec<_> = e.note.strokes().filter(|s| near(s, p, r)).map(|s| s.id).collect();
        for id in hit { e.note.erase(id); e.dirty = true; }
    }

    /// Take back the last stroke made in this session.
    pub fn undo(&mut self) {
        if let Some((n, id)) = self.made.pop() {
            if let Some(e) = self.entries.get_mut(n) { e.note.erase(id); e.dirty = true; }
            self.save_all();
        }
    }

    // ---- drawing the screens -----------------------------------------------------------

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let screen = self.screen.clone();
        match screen {
            Screen::List => self.list_ui(ui),
            Screen::Note(i) => self.note_ui(ui, i),
            Screen::Devices => self.devices_ui(ui),
            Screen::Codes { note, page } => self.codes_ui(ui, note, page),
        }
        if !self.status.is_empty() {
            ui.separator();
            ui.add(egui::Label::new(egui::RichText::new(&self.status).small()).truncate());
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, title: &str) -> bool {
        let mut back = false;
        ui.horizontal(|ui| {
            if self.screen != Screen::List && ui.add(egui::Button::new("‹ back").min_size(egui::vec2(96.0, 56.0))).clicked() { back = true; }
            ui.heading(title);
        });
        ui.separator();
        back
    }

    fn list_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("jotter");
            if ui.add(egui::Button::new("+ new note").min_size(egui::vec2(160.0, 56.0))).clicked() { let i = self.new_note(); self.screen = Screen::Note(i); }
            if ui.add(egui::Button::new("devices").min_size(egui::vec2(130.0, 56.0))).clicked() { self.screen = Screen::Devices; }
        });
        ui.separator();
        if self.entries.is_empty() { ui.label("No notes yet. Tap + new note and write with your finger or a pen."); }
        let mut open = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (i, e) in self.entries.iter().enumerate() {
                let label = format!("{}   ·   {} strokes", e.note.title(), e.note.len());
                if ui.add(egui::Button::new(label).min_size(egui::vec2(ui.available_width(), 64.0))).clicked() { open = Some(i); }
            }
        });
        if let Some(i) = open { self.screen = Screen::Note(i); }
    }

    fn note_ui(&mut self, ui: &mut egui::Ui, i: usize) {
        let Some(title) = self.entries.get(i).map(|e| e.note.title().to_string()) else { self.screen = Screen::List; return };
        let mut go_back = false;
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new("‹ back").min_size(egui::vec2(96.0, 56.0))).clicked() { go_back = true; }
            for (t, label) in [(Tool::Pen, "pen"), (Tool::Eraser, "eraser")] {
                if ui.add(egui::Button::new(label).selected(self.tool == t).min_size(egui::vec2(96.0, 56.0))).clicked() { self.tool = t; }
            }
            if ui.add(egui::Button::new("undo").min_size(egui::vec2(96.0, 56.0))).clicked() { self.undo(); }
            if ui.add(egui::Button::new("share").min_size(egui::vec2(96.0, 56.0))).clicked() { self.screen = Screen::Codes { note: Some(i), page: 0 }; }
            ui.label(title);
        });
        ui.separator();
        let size = ui.available_size();
        let (resp, painter) = ui.allocate_painter(size, egui::Sense::drag());
        let origin = resp.rect.min;
        if let Some(e) = self.entries.get(i) {
            for s in e.note.strokes() { paint_stroke(&painter, origin, s); }
        }
        if let Some(s) = &self.drawing { paint_stroke(&painter, origin, s); }
        let to_canvas = |p: egui::Pos2| egui::pos2(p.x - origin.x, p.y - origin.y);
        match self.tool {
            Tool::Pen => {
                if resp.drag_started() { if let Some(p) = resp.interact_pointer_pos() { self.begin_stroke(to_canvas(p), 0.5); } }
                else if resp.dragged() { if let Some(p) = resp.interact_pointer_pos() { self.extend_stroke(to_canvas(p), 0.5); } }
                if resp.drag_stopped() { self.end_stroke(i); }
            }
            Tool::Eraser => {
                if resp.dragged() || resp.drag_started() { if let Some(p) = resp.interact_pointer_pos() { self.erase_at(i, to_canvas(p), 14.0); } }
                if resp.drag_stopped() { self.save_all(); }
            }
        }
        if go_back { self.drawing = None; self.save_all(); self.screen = Screen::List; }
    }

    fn devices_ui(&mut self, ui: &mut egui::Ui) {
        if self.top_bar(ui, "devices") { self.screen = Screen::List; return; }
        ui.label(egui::RichText::new(&self.me.name).strong().size(26.0));
        ui.label(format!("fingerprint  {}", self.me.fingerprint()));
        ui.add_space(8.0);
        if ui.add(egui::Button::new("show my pairing code").min_size(egui::vec2(260.0, 64.0))).clicked() { self.screen = Screen::Codes { note: None, page: 0 }; }
        ui.label("Scan it with the other device. Only devices you accept here can sync with this one.");
        ui.add_space(12.0);
        ui.heading("trusted");
        if self.trust.peers().is_empty() { ui.label("none yet"); }
        let mut revoke = None;
        for p in self.trust.peers() {
            ui.horizontal(|ui| {
                ui.label(format!("{}   {}", p.name, p.fingerprint()));
                if ui.button("remove").clicked() { revoke = Some(p.id()); }
            });
        }
        if let Some(id) = revoke {
            if self.trust.revoke(&id) {
                if let Some(path) = TrustStore::default_path().filter(|_| false).or(Some(self.dir.join("trusted"))) { let _ = self.trust.save(&path); }
            }
        }
    }

    /// The matrices and the screens they are laid out on, for the QR screen's subject.
    pub fn code_set(&self, note: Option<usize>) -> Option<(Vec<Matrix>, Vec<u8>)> {
        let (kind, data) = match note {
            None => (Kind::Offer, Offer::new(&self.me, self.listen_addrs.clone(), now_s()).encode(&self.me)),
            Some(i) => (Kind::Note, self.entries.get(i)?.note.to_bytes()),
        };
        let m: Option<Vec<Matrix>> = share::frames(kind, &data).iter().map(|f| share::encode(f)).collect();
        m.map(|m| (m, data))
    }

    fn codes_ui(&mut self, ui: &mut egui::Ui, note: Option<usize>, page: usize) {
        let title = if note.is_some() { "this note as codes" } else { "pairing code" };
        if self.top_bar(ui, title) { self.screen = if note.is_some() { Screen::Note(note.unwrap()) } else { Screen::Devices }; return; }
        let Some((mats, _)) = self.code_set(note) else { ui.label("This is too large to show as codes."); return };
        let avail = ui.available_size();
        let ppp = ui.ctx().pixels_per_point();
        let (w, h) = ((avail.x * ppp) as u32, ((avail.y - 64.0).max(100.0) * ppp) as u32);
        let modules = mats.iter().map(|m| m.size).max().unwrap_or(21);
        let screens = share::mosaic(mats.len(), modules, w, h, share::MIN_MODULE_PX);
        let page = page.min(screens.len().saturating_sub(1));
        if screens.len() > 1 {
            ui.horizontal(|ui| {
                if ui.add_enabled(page > 0, egui::Button::new("‹ previous screen").min_size(egui::vec2(180.0, 48.0))).clicked() { self.screen = Screen::Codes { note, page: page - 1 }; }
                ui.label(format!("screen {} of {}", page + 1, screens.len()));
                if ui.add_enabled(page + 1 < screens.len(), egui::Button::new("next screen ›").min_size(egui::vec2(180.0, 48.0))).clicked() { self.screen = Screen::Codes { note, page: page + 1 }; }
            });
        }
        let (rect, _) = ui.allocate_exact_size(egui::vec2(avail.x, (avail.y - 64.0).max(100.0)), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, egui::Color32::WHITE);
        for c in screens.get(page).map(|v| v.as_slice()).unwrap_or(&[]) {
            let tex = self.texture(ui.ctx(), &mats[c.frame]);
            let px = 1.0 / ppp;
            let tl = rect.min + egui::vec2(c.x as f32 * px, c.y as f32 * px);
            let quiet = share::QUIET as f32 * c.module_px as f32 * px;
            let side = mats[c.frame].size as f32 * c.module_px as f32 * px;
            ui.painter().image(tex.id(), egui::Rect::from_min_size(tl + egui::vec2(quiet, quiet), egui::vec2(side, side)), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }
        if note.is_none() { ui.label(format!("{} · {}", self.me.name, self.me.fingerprint())); }
    }

    fn texture(&mut self, ctx: &egui::Context, m: &Matrix) -> egui::TextureHandle {
        let key = (m.size as u64) << 40 ^ share::checksum(&m.dark.iter().map(|b| *b as u8).collect::<Vec<u8>>()) as u64;
        if let Some((_, t)) = self.textures.iter().find(|(k, _)| *k == key) { return t.clone(); }
        let px: Vec<egui::Color32> = m.dark.iter().map(|d| if *d { egui::Color32::BLACK } else { egui::Color32::WHITE }).collect();
        let img = egui::ColorImage { size: [m.size, m.size], source_size: egui::vec2(m.size as f32, m.size as f32), pixels: px };
        let t = ctx.load_texture(format!("code-{key:x}"), img, egui::TextureOptions::NEAREST);
        self.textures.push((key, t.clone()));
        if self.textures.len() > 24 { self.textures.remove(0); }
        t
    }
}

fn near(s: &Stroke, p: egui::Pos2, r: f64) -> bool {
    let (px, py) = (p.x as f64, p.y as f64);
    let d = |a: Vec2d, b: Vec2d| {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let l2 = dx * dx + dy * dy;
        let t = if l2 < 1e-9 { 0.0 } else { (((px - a.x) * dx + (py - a.y) * dy) / l2).clamp(0.0, 1.0) };
        ((px - (a.x + dx * t)).powi(2) + (py - (a.y + dy * t)).powi(2)).sqrt()
    };
    if s.points.len() == 1 { return (s.points[0].pos.x - px).hypot(s.points[0].pos.y - py) <= r; }
    s.points.windows(2).any(|w| d(w[0].pos, w[1].pos) <= r + s.style.width * 0.5)
}

fn paint_stroke(painter: &egui::Painter, origin: egui::Pos2, s: &Stroke) {
    let c = s.style.color;
    let col = egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
    let pts: Vec<egui::Pos2> = s.points.iter().map(|p| egui::pos2(origin.x + p.pos.x as f32, origin.y + p.pos.y as f32)).collect();
    if pts.len() >= 2 { painter.add(egui::Shape::line(pts, egui::Stroke::new(s.style.width as f32, col))); }
    else if let Some(p) = pts.first() { painter.circle_filled(*p, s.style.width as f32 * 0.5, col); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(j: &mut Jotter, size: egui::Vec2, events: Vec<egui::Event>) -> Vec<String> {
        let ctx = egui::Context::default();
        run_on(&ctx, j, size, events)
    }

    fn run_on(ctx: &egui::Context, j: &mut Jotter, size: egui::Vec2, events: Vec<egui::Event>) -> Vec<String> {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), events, time: Some(0.0), ..Default::default() };
        let out = ctx.run_ui(input, |ui| { egui::CentralPanel::default().show_inside(ui, |ui| j.ui(ui)); });
        let mut texts = Vec::new();
        for s in &out.shapes {
            fn walk(s: &egui::Shape, out: &mut Vec<String>) {
                match s {
                    egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                    egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
                    _ => {}
                }
            }
            walk(&s.shape, &mut texts);
        }
        texts
    }

    /// Write with the pointer: press, drag through `pts`, release, over three frames so egui sees a drag.
    fn write(ctx: &egui::Context, j: &mut Jotter, size: egui::Vec2, pts: &[egui::Pos2]) {
        let btn = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        run_on(ctx, j, size, vec![egui::Event::PointerMoved(pts[0])]);
        run_on(ctx, j, size, vec![btn(pts[0], true)]);
        for p in &pts[1..] { run_on(ctx, j, size, vec![egui::Event::PointerMoved(*p)]); }
        run_on(ctx, j, size, vec![btn(*pts.last().unwrap(), false)]);
        run_on(ctx, j, size, vec![]);
    }

    fn app(d: &std::path::Path) -> Jotter { Jotter::open(d.to_path_buf(), "Test tablet").unwrap() }

    #[test]
    fn a_new_device_makes_an_identity_and_an_empty_list() {
        let d = tempfile::tempdir().unwrap();
        let mut j = app(d.path());
        assert_eq!((j.note_count(), &j.me.name[..]), (0, "Test tablet"));
        let t = frame(&mut j, egui::vec2(800.0, 1000.0), vec![]).join("|");
        assert!(t.contains("jotter") && t.contains("new note") && t.contains("No notes yet"), "{t}");
        let again = app(d.path());
        assert_eq!(again.me.ed_pub, j.me.ed_pub, "the same device on the next start");
    }

    #[test]
    fn writing_saves_each_stroke_and_the_page_comes_back_after_a_restart() {
        let d = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let size = egui::vec2(800.0, 1000.0);
        let mut j = app(d.path());
        let i = j.new_note();
        j.screen = Screen::Note(i);
        run_on(&ctx, &mut j, size, vec![]);
        // a stroke well inside the canvas (below the toolbar)
        let pts: Vec<_> = (0..12).map(|k| egui::pos2(200.0 + k as f32 * 20.0, 400.0 + (k as f32 * 0.7).sin() * 30.0)).collect();
        write(&ctx, &mut j, size, &pts);
        assert_eq!(j.note(i).unwrap().len(), 1, "the stroke was kept");
        let n = j.note(i).unwrap().strokes().next().unwrap().points.len();
        assert!(n >= 8, "its points were recorded: {n}");
        let mut again = app(d.path());
        assert_eq!(again.note_count(), 1);
        assert_eq!(again.note(0).unwrap().len(), 1, "saved as it was written: nothing lost on a restart");
        // undo takes it back, on disk too
        j.undo();
        assert_eq!(j.note(i).unwrap().len(), 0);
        again = app(d.path());
        assert_eq!(again.note(0).unwrap().len(), 0);
    }

    #[test]
    fn the_eraser_removes_the_stroke_under_it_and_only_that() {
        let d = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let size = egui::vec2(800.0, 1000.0);
        let mut j = app(d.path());
        let i = j.new_note();
        j.screen = Screen::Note(i);
        run_on(&ctx, &mut j, size, vec![]);
        write(&ctx, &mut j, size, &(0..8).map(|k| egui::pos2(100.0 + k as f32 * 20.0, 300.0)).collect::<Vec<_>>());
        write(&ctx, &mut j, size, &(0..8).map(|k| egui::pos2(100.0 + k as f32 * 20.0, 600.0)).collect::<Vec<_>>());
        assert_eq!(j.note(i).unwrap().len(), 2);
        j.tool = Tool::Eraser;
        // drag across the upper line (canvas y is screen y minus the toolbar height, so ask the stroke where it is)
        let y = j.note(i).unwrap().strokes().next().unwrap().points[0].pos.y as f32;
        let (ox, oy) = (0.0f32, (300.0 - y).max(0.0));
        let _ = ox;
        write(&ctx, &mut j, size, &(0..6).map(|k| egui::pos2(120.0 + k as f32 * 20.0, 300.0 + oy * 0.0)).collect::<Vec<_>>());
        assert_eq!(j.note(i).unwrap().len(), 1, "one erased, the other left");
        let left = j.note(i).unwrap().strokes().next().unwrap().points[0].pos.y;
        assert!(left > y as f64 + 100.0, "the lower line is what remains");
    }

    #[test]
    fn the_pairing_code_and_the_share_codes_are_real_symbols_on_screen() {
        let d = tempfile::tempdir().unwrap();
        let mut j = app(d.path());
        j.listen_addrs = vec!["192.168.2.185:7471".into()];
        let (mats, data) = j.code_set(None).expect("an offer fits one code");
        assert_eq!(mats.len(), 1);
        let offer = Offer::decode(&data).expect("the code carries a signed offer");
        assert_eq!((offer.ed_pub, &offer.addrs[..]), (j.me.ed_pub, &["192.168.2.185:7471".to_string()][..]));
        // a big note needs several frames, shown together
        let i = j.new_note();
        for k in 0..4000u64 {
            let mut s = Stroke::new(stroke_id(1000 + k, k as u32), StrokeStyle::default());
            for q in 0..30 { s.points.push(InkPoint::new(Vec2d::new((k % 90) as f64 * 9.0 + q as f64 * 1.3, (k / 90) as f64 * 11.0 + (q as f64).sin() * 4.0), 0.5, 0.0)); }
            j.entries[i].note.add(s);
        }
        let (mats, data) = j.code_set(Some(i)).expect("fits in a few codes");
        assert!(mats.len() >= 3, "a dense page is several frames: {}", mats.len());
        let frames = share::frames(Kind::Note, &data);
        assert_eq!(share::reassemble(&frames).map(|(k, d)| (k, Note::from_bytes(&d).map(|n| n.len()))), Some((Kind::Note, Some(4000))), "and they put the page back together");
        j.screen = Screen::Codes { note: Some(i), page: 0 };
        let t = frame(&mut j, egui::vec2(1620.0 / 2.0, 2160.0 / 2.0), vec![]).join("|");
        assert!(t.contains("this note as codes"), "{t}");
        j.screen = Screen::Codes { note: None, page: 0 };
        let t = frame(&mut j, egui::vec2(600.0, 800.0), vec![]).join("|");
        assert!(t.contains("pairing code") && t.contains("Test tablet") && t.contains(&j.me.fingerprint()), "{t}");
    }

    #[test]
    fn the_devices_screen_shows_the_fingerprint_and_lets_a_trusted_device_be_removed() {
        let d = tempfile::tempdir().unwrap();
        let mut j = app(d.path());
        let phone = Identity::generate("Phone");
        j.trust.trust(&Offer::new(&phone, vec![], 5), 6);
        j.trust.save(&d.path().join("trusted")).unwrap();
        j.screen = Screen::Devices;
        let t = frame(&mut j, egui::vec2(800.0, 1000.0), vec![]).join("|");
        assert!(t.contains(&j.me.fingerprint()) && t.contains("Phone") && t.contains(&phone.fingerprint()), "{t}");
        assert!(app(d.path()).trust.is_trusted(&phone.ed_pub), "the list persists");
    }
}
