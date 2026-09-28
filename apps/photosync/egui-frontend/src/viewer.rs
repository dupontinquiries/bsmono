//! viewer — one photo, full size, and everything about it.
//!
//! Swipe (or arrow keys) moves through the sequence the grid was showing;
//! the thumbnail is drawn scaled up the instant a photo opens and the full
//! decode replaces it when the worker delivers it, so paging never waits
//! on a 12-megapixel JPEG. The info panel under the picture holds what
//! the library knows — tags to edit, the derived tags, the ascii the core
//! made for a language model, who it is shared with — and every button
//! returns an action for the shell to carry out.
//!
//! The swipe rule and the fit rule are pure and tested; the drawing is
//! the only part that needs a frame.

use crate::library::{Library, Photo};
use crate::share::{Contact, PhotoShare};
use crate::textures::Textures;
use std::sync::mpsc::{channel, Receiver};

/// A drag shorter than this (in points) is a tap, not a page.
pub const SWIPE_MIN_PX: f32 = 48.0;
/// The long side the full view is decoded to. Above this a phone's GPU
/// is holding pixels it cannot show.
pub const FULL_PX: u32 = 2048;

/// What the viewer asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewerAction {
    Close,
    AddTag(usize, String),
    RemoveTag(usize, String),
    Share(usize, String),
    Revoke(usize, String),
    /// Show photos like this one.
    Like(usize),
    /// Move this photo to the trash folder.
    Trash(usize),
}

/// The open photo and its neighbours.
pub struct Viewer {
    /// Library indices, in the order the grid showed them.
    pub seq: Vec<usize>,
    pub pos: usize,
    pub info: bool,
    pub tag_text: String,
    /// The trash button's first tap; the second one acts.
    pub confirm_trash: bool,
    full: Option<(String, egui::TextureHandle)>,
    full_rx: Option<Receiver<(String, Result<egui::ColorImage, String>)>>,
    /// Horizontal drag accumulated this gesture.
    drag_x: f32,
    pub note: Option<String>,
}

impl Viewer {
    pub fn open(seq: Vec<usize>, pos: usize) -> Viewer {
        let pos = pos.min(seq.len().saturating_sub(1));
        Viewer { seq, pos, info: false, tag_text: String::new(),
                 confirm_trash: false, full: None, full_rx: None, drag_x: 0.0, note: None }
    }

    /// The current library index.
    pub fn current(&self) -> Option<usize> { self.seq.get(self.pos).copied() }
    pub fn has_next(&self) -> bool { self.pos + 1 < self.seq.len() }
    pub fn has_prev(&self) -> bool { self.pos > 0 }

    pub fn next(&mut self) -> bool { if self.has_next() { self.pos += 1; self.moved(); true } else { false } }
    pub fn prev(&mut self) -> bool { if self.has_prev() { self.pos -= 1; self.moved(); true } else { false } }

    fn moved(&mut self) { self.confirm_trash = false; self.tag_text.clear(); self.note = None; }

    /// "3 / 40".
    pub fn position_text(&self) -> String { format!("{} / {}", self.pos + 1, self.seq.len()) }

    /// A photo left the library (trashed): drop it from the sequence and
    /// stay in place, or close when nothing is left.
    pub fn remove_index(&mut self, i: usize) -> bool {
        self.seq.retain(|x| *x != i);
        // Indices above the removed one shift down by one in the library.
        for x in &mut self.seq { if *x > i { *x -= 1; } }
        if self.seq.is_empty() { return false; }
        self.pos = self.pos.min(self.seq.len() - 1);
        self.moved();
        true
    }

    /// Ask the worker for the full decode of the current photo, unless it
    /// is already here or on its way.
    fn request_full(&mut self, path: &str) {
        if self.full.as_ref().map(|(p, _)| p == path).unwrap_or(false) { return; }
        if self.full_rx.is_some() { return; }
        let (tx, rx) = channel();
        let p = path.to_string();
        std::thread::Builder::new().name("photosync-full".into()).spawn(move || {
            let r = image::open(&p).map(|img| {
                let img = if img.width().max(img.height()) > FULL_PX { img.thumbnail(FULL_PX, FULL_PX) } else { img };
                let rgba = img.to_rgba8();
                egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
            }).map_err(|e| e.to_string());
            let _ = tx.send((p, r));
        }).ok();
        self.full_rx = Some(rx);
    }

    fn poll_full(&mut self, ctx: &egui::Context, want: &str) {
        let Some(rx) = &self.full_rx else { return };
        match rx.try_recv() {
            Ok((path, Ok(img))) => {
                self.full_rx = None;
                if path == want {
                    self.full = Some((path.clone(), ctx.load_texture(format!("full:{path}"), img, egui::TextureOptions::LINEAR)));
                }
            }
            Ok((path, Err(e))) => { self.full_rx = None; if path == want { self.note = Some(format!("could not decode: {e}")); } }
            Err(std::sync::mpsc::TryRecvError::Empty) => { ctx.request_repaint_after(std::time::Duration::from_millis(50)); }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => { self.full_rx = None; }
        }
    }
}

/// Does a horizontal drag page, and which way? +1 is "next" (dragged
/// left), −1 "previous". Distance beats velocity here: a flick is fast
/// but a slow deliberate drag past the threshold must page too.
pub fn swipe_commits(drag_x: f32, width: f32) -> i32 {
    let threshold = SWIPE_MIN_PX.max(width * 0.15);
    if drag_x <= -threshold { 1 } else if drag_x >= threshold { -1 } else { 0 }
}

/// Where a `w × h` picture goes in `area`: as large as fits, centred,
/// never upscaled past 1:1 by more than the area needs (a small photo is
/// shown at the size that fills the width; pixel art is not the case).
pub fn fit_rect(w: f32, h: f32, area: egui::Rect) -> egui::Rect {
    if w <= 0.0 || h <= 0.0 { return area; }
    let scale = (area.width() / w).min(area.height() / h);
    let size = egui::vec2(w * scale, h * scale);
    egui::Rect::from_center_size(area.center(), size)
}

/// Format a byte count the way a person reads one.
pub fn human_size(bytes: u64) -> String {
    if bytes >= 1 << 20 { format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64) }
    else if bytes >= 1 << 10 { format!("{:.0} kB", bytes as f64 / 1024.0) }
    else { format!("{bytes} B") }
}

/// Draw the viewer. Returns what was asked for.
pub fn viewer_ui(ui: &mut egui::Ui, v: &mut Viewer, lib: &mut Library, textures: &mut Textures,
                 share: &PhotoShare, contacts: &[Contact], compact: bool,
                 c: &uikit_egui::PaletteColors) -> Vec<ViewerAction> {
    let mut out = Vec::new();
    let Some(i) = v.current() else { return vec![ViewerAction::Close] };
    let Some(photo_path) = lib.photos.get(i).map(|p| p.path.clone()) else { return vec![ViewerAction::Close] };
    v.request_full(&photo_path);
    v.poll_full(ui.ctx(), &photo_path);

    // Keys: Escape closes, arrows page, i toggles the panel.
    let (esc, left, right, info_key) = ui.ctx().input(|inp| (
        inp.key_pressed(egui::Key::Escape), inp.key_pressed(egui::Key::ArrowLeft),
        inp.key_pressed(egui::Key::ArrowRight), inp.key_pressed(egui::Key::I)));
    if esc { out.push(ViewerAction::Close); }
    if left { v.prev(); }
    if right { v.next(); }
    if info_key && !ui.ctx().egui_wants_keyboard_input() { v.info = !v.info; }

    // Top bar.
    ui.horizontal(|ui| {
        if ui.button("← Back").clicked() { out.push(ViewerAction::Close); }
        let name = lib.photos.get(i).map(|p| p.name().to_string()).unwrap_or_default();
        ui.add(egui::Label::new(egui::RichText::new(name).strong().color(c.text)).truncate());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.selectable_label(v.info, "info").on_hover_text("i").clicked() { v.info = !v.info; }
            ui.colored_label(c.text_weak, v.position_text());
        });
    });

    // The picture: what is left after the panel takes its share.
    let panel_h = if v.info { if compact { ui.available_height() * 0.5 } else { 0.0 } } else { 0.0 };
    let avail = ui.available_rect_before_wrap();
    let side_panel = v.info && !compact;
    let pic_area = if side_panel {
        egui::Rect::from_min_size(avail.min, egui::vec2(avail.width() * 0.62, avail.height()))
    } else {
        egui::Rect::from_min_size(avail.min, egui::vec2(avail.width(), avail.height() - panel_h))
    };
    let resp = ui.allocate_rect(pic_area, egui::Sense::click_and_drag());
    ui.painter().rect_filled(pic_area, 0.0, egui::Color32::from_rgb(8, 8, 10));
    let tex: Option<(egui::TextureHandle, [usize; 2])> = match &v.full {
        Some((p, t)) if *p == photo_path => Some((t.clone(), t.size())),
        _ => lib.photos.get_mut(i).and_then(|p| textures.get(ui.ctx(), p)).map(|t| { let s = t.size(); (t, s) }),
    };
    match tex {
        Some((t, [w, h])) => {
            let r = fit_rect(w as f32, h as f32, pic_area.shrink(4.0));
            ui.painter().image(t.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }
        None => { ui.painter().text(pic_area.center(), egui::Align2::CENTER_CENTER, "decoding…", egui::FontId::proportional(16.0), c.text_weak); }
    }
    if v.full.as_ref().map(|(p, _)| p != &photo_path).unwrap_or(true) && v.full_rx.is_some() {
        ui.painter().text(pic_area.left_bottom() + egui::vec2(8.0, -8.0), egui::Align2::LEFT_BOTTOM,
            "loading full size…", egui::FontId::proportional(12.0), c.text_weak);
    }
    // Swipe.
    if resp.dragged() { v.drag_x += resp.drag_delta().x; }
    if resp.drag_stopped() {
        match swipe_commits(v.drag_x, pic_area.width()) { 1 => { v.next(); } -1 => { v.prev(); } _ => {} }
        v.drag_x = 0.0;
    }
    if resp.clicked() && !compact {
        // A tap on the right third pages forward, the left third back —
        // the desktop convention, and cheaper than hunting for arrows.
        if let Some(p) = resp.interact_pointer_pos() {
            let f = (p.x - pic_area.left()) / pic_area.width();
            if f > 0.66 { v.next(); } else if f < 0.33 { v.prev(); }
        }
    }
    if let Some(n) = &v.note {
        ui.painter().text(pic_area.center_bottom() - egui::vec2(0.0, 24.0), egui::Align2::CENTER_BOTTOM, n,
            egui::FontId::proportional(13.0), c.warning);
    }

    if v.info {
        let panel_rect = if side_panel {
            egui::Rect::from_min_max(egui::pos2(pic_area.right() + 8.0, avail.top()), avail.max)
        } else {
            egui::Rect::from_min_max(egui::pos2(avail.left(), pic_area.bottom() + 4.0), avail.max)
        };
        let mut panel = ui.new_child(egui::UiBuilder::new().max_rect(panel_rect).layout(egui::Layout::top_down(egui::Align::Min)));
        egui::ScrollArea::vertical().id_salt("viewer-info").auto_shrink([false, false]).show(&mut panel, |ui| {
            if let Some(p) = lib.photos.get(i) {
                out.extend(info_ui(ui, v, i, p, share, contacts, c));
            }
        });
    }
    out
}

/// The info panel. Pure data in, actions out.
fn info_ui(ui: &mut egui::Ui, v: &mut Viewer, i: usize, p: &Photo, share: &PhotoShare,
           contacts: &[Contact], c: &uikit_egui::PaletteColors) -> Vec<ViewerAction> {
    let mut out = Vec::new();
    egui::Grid::new("viewer-facts").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
        ui.colored_label(c.text_weak, "modified");
        ui.colored_label(c.text, p.day_label());
        ui.end_row();
        ui.colored_label(c.text_weak, "size");
        ui.colored_label(c.text, format!("{}×{}, {}", p.w, p.h, human_size(p.size)));
        ui.end_row();
        ui.colored_label(c.text_weak, "path");
        ui.add(egui::Label::new(egui::RichText::new(&p.path).small().color(c.text_weak)).truncate());
        ui.end_row();
    });
    ui.add_space(6.0);
    ui.colored_label(c.text, egui::RichText::new("Tags").strong());
    ui.horizontal_wrapped(|ui| {
        for t in p.written_tags() {
            if ui.small_button(format!("{t} ✕")).on_hover_text("remove").clicked() {
                out.push(ViewerAction::RemoveTag(i, t.to_string()));
            }
        }
        if p.written_tags().is_empty() { ui.colored_label(c.text_weak, egui::RichText::new("none written").small()); }
    });
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut v.tag_text).hint_text("add a tag").desired_width(140.0));
        let entered = r.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter));
        if (ui.small_button("Add").clicked() || entered) && !v.tag_text.trim().is_empty() {
            out.push(ViewerAction::AddTag(i, v.tag_text.trim().to_string()));
            v.tag_text.clear();
        }
    });
    let auto = p.auto_tags();
    if !auto.is_empty() {
        ui.colored_label(c.text_weak, egui::RichText::new(format!("derived: {}", auto.join(", "))).small());
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Find similar").clicked() { out.push(ViewerAction::Like(i)); }
        let label = if v.confirm_trash { "Really move to trash?" } else { "Trash" };
        if ui.button(label).clicked() {
            if v.confirm_trash { out.push(ViewerAction::Trash(i)); v.confirm_trash = false; }
            else { v.confirm_trash = true; }
        }
    });
    ui.add_space(6.0);
    ui.colored_label(c.text, egui::RichText::new("Share with").strong());
    if contacts.is_empty() {
        ui.colored_label(c.text_weak, egui::RichText::new("no snapclone contacts on this device").small());
    }
    for ct in contacts {
        ui.horizontal(|ui| {
            let on = share.is_shared_with(&p.path, &ct.sc_id);
            ui.colored_label(c.text, ct.label());
            if on {
                if ui.small_button("Stop sharing").clicked() { out.push(ViewerAction::Revoke(i, ct.sc_id.clone())); }
                ui.colored_label(c.success, egui::RichText::new("· sharing").small());
            } else if ui.small_button("Share").clicked() {
                out.push(ViewerAction::Share(i, ct.sc_id.clone()));
            }
        });
    }
    ui.add_space(6.0);
    ui.collapsing("ascii (what a language model sees)", |ui| {
        ui.add(egui::Label::new(egui::RichText::new(&p.entry.ascii).monospace().small().color(c.text_weak)));
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_swipe_pages_only_past_the_threshold_and_in_the_dragged_direction() {
        assert_eq!(swipe_commits(-10.0, 400.0), 0, "a tap-sized wobble is not a page");
        assert_eq!(swipe_commits(-70.0, 400.0), 1, "dragged left: next");
        assert_eq!(swipe_commits(70.0, 400.0), -1, "dragged right: previous");
        // On a wide screen the threshold scales with the width.
        assert_eq!(swipe_commits(-100.0, 1200.0), 0);
        assert_eq!(swipe_commits(-200.0, 1200.0), 1);
    }

    #[test]
    fn the_picture_fits_the_area_and_keeps_its_aspect() {
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        let r = fit_rect(4000.0, 3000.0, area);
        assert!((r.width() - 400.0).abs() < 1e-3 && (r.height() - 300.0).abs() < 1e-3);
        let tall = fit_rect(300.0, 600.0, area);
        assert!((tall.height() - 300.0).abs() < 1e-3);
        assert!((tall.width() - 150.0).abs() < 1e-3);
        assert_eq!(tall.center(), area.center(), "centred");
    }

    #[test]
    fn paging_stops_at_the_ends_and_removal_keeps_the_place() {
        let mut v = Viewer::open(vec![5, 7, 9], 1);
        assert_eq!(v.current(), Some(7));
        assert_eq!(v.position_text(), "2 / 3");
        assert!(v.next());
        assert!(!v.next(), "no wrap past the end");
        assert_eq!(v.current(), Some(9));
        // Trash the current (library index 9): the viewer stays at the
        // last remaining photo, and indices above 9 would have shifted.
        assert!(v.remove_index(9));
        assert_eq!(v.current(), Some(7));
        assert!(v.remove_index(5));
        assert_eq!(v.seq, vec![6], "7 shifted down because 5 left the library");
        assert!(!v.remove_index(6), "nothing left: close");
    }

    #[test]
    fn sizes_read_like_a_person_wrote_them() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(20 * 1024), "20 kB");
        assert_eq!(human_size(3 * 1024 * 1024 + 200_000), "3.2 MB");
    }
}
//! viewer — one photo, full size, and everything about it.
//!
//! Swipe (or arrow keys) moves through the sequence the grid was showing;
//! the thumbnail is drawn scaled up the instant a photo opens and the full
//! decode replaces it when the worker delivers it, so paging never waits
//! on a 12-megapixel JPEG. The info panel under the picture holds what
//! the library knows — tags to edit, the derived tags, the ascii the core
//! made for a language model, who it is shared with — and every button
//! returns an action for the shell to carry out.
//!
//! The swipe rule and the fit rule are pure and tested; the drawing is
//! the only part that needs a frame.

use crate::library::{Library, Photo};
use crate::share::{Contact, PhotoShare};
use crate::textures::Textures;
use std::sync::mpsc::{channel, Receiver};

/// A drag shorter than this (in points) is a tap, not a page.
pub const SWIPE_MIN_PX: f32 = 48.0;
/// The long side the full view is decoded to. Above this a phone's GPU
/// is holding pixels it cannot show.
pub const FULL_PX: u32 = 2048;

/// What the viewer asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewerAction {
    Close,
    AddTag(usize, String),
    RemoveTag(usize, String),
    Share(usize, String),
    Revoke(usize, String),
    /// Show photos like this one.
    Like(usize),
    /// Move this photo to the trash folder.
    Trash(usize),
}

/// The open photo and its neighbours.
pub struct Viewer {
    /// Library indices, in the order the grid showed them.
    pub seq: Vec<usize>,
    pub pos: usize,
    pub info: bool,
    pub tag_text: String,
    /// The trash button's first tap; the second one acts.
    pub confirm_trash: bool,
    full: Option<(String, egui::TextureHandle)>,
    full_rx: Option<Receiver<(String, Result<egui::ColorImage, String>)>>,
    /// Horizontal drag accumulated this gesture.
    drag_x: f32,
    pub note: Option<String>,
}

impl Viewer {
    pub fn open(seq: Vec<usize>, pos: usize) -> Viewer {
        let pos = pos.min(seq.len().saturating_sub(1));
        Viewer { seq, pos, info: false, tag_text: String::new(),
                 confirm_trash: false, full: None, full_rx: None, drag_x: 0.0, note: None }
    }

    /// The current library index.
    pub fn current(&self) -> Option<usize> { self.seq.get(self.pos).copied() }
    pub fn has_next(&self) -> bool { self.pos + 1 < self.seq.len() }
    pub fn has_prev(&self) -> bool { self.pos > 0 }

    pub fn next(&mut self) -> bool { if self.has_next() { self.pos += 1; self.moved(); true } else { false } }
    pub fn prev(&mut self) -> bool { if self.has_prev() { self.pos -= 1; self.moved(); true } else { false } }

    fn moved(&mut self) { self.confirm_trash = false; self.tag_text.clear(); self.note = None; }

    /// "3 / 40".
    pub fn position_text(&self) -> String { format!("{} / {}", self.pos + 1, self.seq.len()) }

    /// A photo left the library (trashed): drop it from the sequence and
    /// stay in place, or close when nothing is left.
    pub fn remove_index(&mut self, i: usize) -> bool {
        self.seq.retain(|x| *x != i);
        // Indices above the removed one shift down by one in the library.
        for x in &mut self.seq { if *x > i { *x -= 1; } }
        if self.seq.is_empty() { return false; }
        self.pos = self.pos.min(self.seq.len() - 1);
        self.moved();
        true
    }

    /// Ask the worker for the full decode of the current photo, unless it
    /// is already here or on its way.
    fn request_full(&mut self, path: &str) {
        if self.full.as_ref().map(|(p, _)| p == path).unwrap_or(false) { return; }
        if self.full_rx.is_some() { return; }
        let (tx, rx) = channel();
        let p = path.to_string();
        std::thread::Builder::new().name("photosync-full".into()).spawn(move || {
            let r = image::open(&p).map(|img| {
                let img = if img.width().max(img.height()) > FULL_PX { img.thumbnail(FULL_PX, FULL_PX) } else { img };
                let rgba = img.to_rgba8();
                egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
            }).map_err(|e| e.to_string());
            let _ = tx.send((p, r));
        }).ok();
        self.full_rx = Some(rx);
    }

    fn poll_full(&mut self, ctx: &egui::Context, want: &str) {
        let Some(rx) = &self.full_rx else { return };
        match rx.try_recv() {
            Ok((path, Ok(img))) => {
                self.full_rx = None;
                if path == want {
                    self.full = Some((path.clone(), ctx.load_texture(format!("full:{path}"), img, egui::TextureOptions::LINEAR)));
                }
            }
            Ok((path, Err(e))) => { self.full_rx = None; if path == want { self.note = Some(format!("could not decode: {e}")); } }
            Err(std::sync::mpsc::TryRecvError::Empty) => { ctx.request_repaint_after(std::time::Duration::from_millis(50)); }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => { self.full_rx = None; }
        }
    }
}

/// Does a horizontal drag page, and which way? +1 is "next" (dragged
/// left), −1 "previous". Distance beats velocity here: a flick is fast
/// but a slow deliberate drag past the threshold must page too.
pub fn swipe_commits(drag_x: f32, width: f32) -> i32 {
    let threshold = SWIPE_MIN_PX.max(width * 0.15);
    if drag_x <= -threshold { 1 } else if drag_x >= threshold { -1 } else { 0 }
}

/// Where a `w × h` picture goes in `area`: as large as fits, centred,
/// never upscaled past 1:1 by more than the area needs (a small photo is
/// shown at the size that fills the width; pixel art is not the case).
pub fn fit_rect(w: f32, h: f32, area: egui::Rect) -> egui::Rect {
    if w <= 0.0 || h <= 0.0 { return area; }
    let scale = (area.width() / w).min(area.height() / h);
    let size = egui::vec2(w * scale, h * scale);
    egui::Rect::from_center_size(area.center(), size)
}

/// Format a byte count the way a person reads one.
pub fn human_size(bytes: u64) -> String {
    if bytes >= 1 << 20 { format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64) }
    else if bytes >= 1 << 10 { format!("{:.0} kB", bytes as f64 / 1024.0) }
    else { format!("{bytes} B") }
}

/// Draw the viewer. Returns what was asked for.
pub fn viewer_ui(ui: &mut egui::Ui, v: &mut Viewer, lib: &mut Library, textures: &mut Textures,
                 share: &PhotoShare, contacts: &[Contact], compact: bool,
                 c: &uikit_egui::PaletteColors) -> Vec<ViewerAction> {
    let mut out = Vec::new();
    let Some(i) = v.current() else { return vec![ViewerAction::Close] };
    let Some(photo_path) = lib.photos.get(i).map(|p| p.path.clone()) else { return vec![ViewerAction::Close] };
    v.request_full(&photo_path);
    v.poll_full(ui.ctx(), &photo_path);

    // Keys: Escape closes, arrows page, i toggles the panel.
    let (esc, left, right, info_key) = ui.ctx().input(|inp| (
        inp.key_pressed(egui::Key::Escape), inp.key_pressed(egui::Key::ArrowLeft),
        inp.key_pressed(egui::Key::ArrowRight), inp.key_pressed(egui::Key::I)));
    if esc { out.push(ViewerAction::Close); }
    if left { v.prev(); }
    if right { v.next(); }
    if info_key && !ui.ctx().egui_wants_keyboard_input() { v.info = !v.info; }

    // Top bar.
    ui.horizontal(|ui| {
        if ui.button("← Back").clicked() { out.push(ViewerAction::Close); }
        let name = lib.photos.get(i).map(|p| p.name().to_string()).unwrap_or_default();
        ui.add(egui::Label::new(egui::RichText::new(name).strong().color(c.text)).truncate());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.selectable_label(v.info, "info").on_hover_text("i").clicked() { v.info = !v.info; }
            ui.colored_label(c.text_weak, v.position_text());
        });
    });

    // The picture: what is left after the panel takes its share.
    let panel_h = if v.info { if compact { ui.available_height() * 0.5 } else { 0.0 } } else { 0.0 };
    let avail = ui.available_rect_before_wrap();
    let side_panel = v.info && !compact;
    let pic_area = if side_panel {
        egui::Rect::from_min_size(avail.min, egui::vec2(avail.width() * 0.62, avail.height()))
    } else {
        egui::Rect::from_min_size(avail.min, egui::vec2(avail.width(), avail.height() - panel_h))
    };
    let resp = ui.allocate_rect(pic_area, egui::Sense::click_and_drag());
    ui.painter().rect_filled(pic_area, 0.0, egui::Color32::from_rgb(8, 8, 10));
    let tex: Option<(egui::TextureHandle, [usize; 2])> = match &v.full {
        Some((p, t)) if *p == photo_path => Some((t.clone(), t.size())),
        _ => lib.photos.get_mut(i).and_then(|p| textures.get(ui.ctx(), p)).map(|t| { let s = t.size(); (t, s) }),
    };
    match tex {
        Some((t, [w, h])) => {
            let r = fit_rect(w as f32, h as f32, pic_area.shrink(4.0));
            ui.painter().image(t.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }
        None => { ui.painter().text(pic_area.center(), egui::Align2::CENTER_CENTER, "decoding…", egui::FontId::proportional(16.0), c.text_weak); }
    }
    if v.full.as_ref().map(|(p, _)| p != &photo_path).unwrap_or(true) && v.full_rx.is_some() {
        ui.painter().text(pic_area.left_bottom() + egui::vec2(8.0, -8.0), egui::Align2::LEFT_BOTTOM,
            "loading full size…", egui::FontId::proportional(12.0), c.text_weak);
    }
    // Swipe.
    if resp.dragged() { v.drag_x += resp.drag_delta().x; }
    if resp.drag_stopped() {
        match swipe_commits(v.drag_x, pic_area.width()) { 1 => { v.next(); } -1 => { v.prev(); } _ => {} }
        v.drag_x = 0.0;
    }
    if resp.clicked() && !compact {
        // A tap on the right third pages forward, the left third back —
        // the desktop convention, and cheaper than hunting for arrows.
        if let Some(p) = resp.interact_pointer_pos() {
            let f = (p.x - pic_area.left()) / pic_area.width();
            if f > 0.66 { v.next(); } else if f < 0.33 { v.prev(); }
        }
    }
    if let Some(n) = &v.note {
        ui.painter().text(pic_area.center_bottom() - egui::vec2(0.0, 24.0), egui::Align2::CENTER_BOTTOM, n,
            egui::FontId::proportional(13.0), c.warning);
    }

    if v.info {
        let panel_rect = if side_panel {
            egui::Rect::from_min_max(egui::pos2(pic_area.right() + 8.0, avail.top()), avail.max)
        } else {
            egui::Rect::from_min_max(egui::pos2(avail.left(), pic_area.bottom() + 4.0), avail.max)
        };
        let mut panel = ui.new_child(egui::UiBuilder::new().max_rect(panel_rect).layout(egui::Layout::top_down(egui::Align::Min)));
        egui::ScrollArea::vertical().id_salt("viewer-info").auto_shrink([false, false]).show(&mut panel, |ui| {
            if let Some(p) = lib.photos.get(i) {
                out.extend(info_ui(ui, v, i, p, share, contacts, c));
            }
        });
    }
    out
}

/// The info panel. Pure data in, actions out.
fn info_ui(ui: &mut egui::Ui, v: &mut Viewer, i: usize, p: &Photo, share: &PhotoShare,
           contacts: &[Contact], c: &uikit_egui::PaletteColors) -> Vec<ViewerAction> {
    let mut out = Vec::new();
    egui::Grid::new("viewer-facts").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
        ui.colored_label(c.text_weak, "modified");
        ui.colored_label(c.text, p.day_label());
        ui.end_row();
        ui.colored_label(c.text_weak, "size");
        ui.colored_label(c.text, format!("{}×{}, {}", p.w, p.h, human_size(p.size)));
        ui.end_row();
        ui.colored_label(c.text_weak, "path");
        ui.add(egui::Label::new(egui::RichText::new(&p.path).small().color(c.text_weak)).truncate());
        ui.end_row();
    });
    ui.add_space(6.0);
    ui.colored_label(c.text, egui::RichText::new("Tags").strong());
    ui.horizontal_wrapped(|ui| {
        for t in p.written_tags() {
            if ui.small_button(format!("{t} ✕")).on_hover_text("remove").clicked() {
                out.push(ViewerAction::RemoveTag(i, t.to_string()));
            }
        }
        if p.written_tags().is_empty() { ui.colored_label(c.text_weak, egui::RichText::new("none written").small()); }
    });
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut v.tag_text).hint_text("add a tag").desired_width(140.0));
        let entered = r.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter));
        if (ui.small_button("Add").clicked() || entered) && !v.tag_text.trim().is_empty() {
            out.push(ViewerAction::AddTag(i, v.tag_text.trim().to_string()));
            v.tag_text.clear();
        }
    });
    let auto = p.auto_tags();
    if !auto.is_empty() {
        ui.colored_label(c.text_weak, egui::RichText::new(format!("derived: {}", auto.join(", "))).small());
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Find similar").clicked() { out.push(ViewerAction::Like(i)); }
        let label = if v.confirm_trash { "Really move to trash?" } else { "Trash" };
        if ui.button(label).clicked() {
            if v.confirm_trash { out.push(ViewerAction::Trash(i)); v.confirm_trash = false; }
            else { v.confirm_trash = true; }
        }
    });
    ui.add_space(6.0);
    ui.colored_label(c.text, egui::RichText::new("Share with").strong());
    if contacts.is_empty() {
        ui.colored_label(c.text_weak, egui::RichText::new("no snapclone contacts on this device").small());
    }
    for ct in contacts {
        ui.horizontal(|ui| {
            let on = share.is_shared_with(&p.path, &ct.sc_id);
            ui.colored_label(c.text, ct.label());
            if on {
                if ui.small_button("Stop sharing").clicked() { out.push(ViewerAction::Revoke(i, ct.sc_id.clone())); }
                ui.colored_label(c.success, egui::RichText::new("· sharing").small());
            } else if ui.small_button("Share").clicked() {
                out.push(ViewerAction::Share(i, ct.sc_id.clone()));
            }
        });
    }
    ui.add_space(6.0);
    ui.collapsing("ascii (what a language model sees)", |ui| {
        ui.add(egui::Label::new(egui::RichText::new(&p.entry.ascii).monospace().small().color(c.text_weak)));
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_swipe_pages_only_past_the_threshold_and_in_the_dragged_direction() {
        assert_eq!(swipe_commits(-10.0, 400.0), 0, "a tap-sized wobble is not a page");
        assert_eq!(swipe_commits(-70.0, 400.0), 1, "dragged left: next");
        assert_eq!(swipe_commits(70.0, 400.0), -1, "dragged right: previous");
        // On a wide screen the threshold scales with the width.
        assert_eq!(swipe_commits(-100.0, 1200.0), 0);
        assert_eq!(swipe_commits(-200.0, 1200.0), 1);
    }

    #[test]
    fn the_picture_fits_the_area_and_keeps_its_aspect() {
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        let r = fit_rect(4000.0, 3000.0, area);
        assert!((r.width() - 400.0).abs() < 1e-3 && (r.height() - 300.0).abs() < 1e-3);
        let tall = fit_rect(300.0, 600.0, area);
        assert!((tall.height() - 300.0).abs() < 1e-3);
        assert!((tall.width() - 150.0).abs() < 1e-3);
        assert_eq!(tall.center(), area.center(), "centred");
    }

    #[test]
    fn paging_stops_at_the_ends_and_removal_keeps_the_place() {
        let mut v = Viewer::open(vec![5, 7, 9], 1);
        assert_eq!(v.current(), Some(7));
        assert_eq!(v.position_text(), "2 / 3");
        assert!(v.next());
        assert!(!v.next(), "no wrap past the end");
        assert_eq!(v.current(), Some(9));
        // Trash the current (library index 9): the viewer stays at the
        // last remaining photo, and indices above 9 would have shifted.
        assert!(v.remove_index(9));
        assert_eq!(v.current(), Some(7));
        assert!(v.remove_index(5));
        assert_eq!(v.seq, vec![6], "7 shifted down because 5 left the library");
        assert!(!v.remove_index(6), "nothing left: close");
    }

    #[test]
    fn sizes_read_like_a_person_wrote_them() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(20 * 1024), "20 kB");
        assert_eq!(human_size(3 * 1024 * 1024 + 200_000), "3.2 MB");
    }
}
