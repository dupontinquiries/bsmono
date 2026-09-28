//! pages — Duplicates, Shared and Library.
//!
//! Each is a function over the shell's data that returns what was asked
//! for. The decisions worth testing (which of a duplicate group to keep,
//! what the shared page lists for a contact, what the library page says
//! about a scan) are on the pure types; the functions draw them.

use crate::library::Library;
use crate::share::{Contact, PhotoShare};
use crate::textures::Textures;
use crate::viewer::human_size;

/// What the duplicates page asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DedupAction {
    /// Keep `keep`, trash the rest of its group.
    Resolve { keep: usize, trash: Vec<usize> },
    Open(usize),
}

/// The duplicate groups, cached between library changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DedupPage {
    pub groups: Vec<Vec<usize>>,
    /// True when the library changed since `groups` was computed.
    pub stale: bool,
    pub note: Option<String>,
}

impl DedupPage {
    pub fn refresh(&mut self, lib: &Library) {
        self.groups = lib.duplicate_groups();
        self.stale = false;
    }

    /// How many files the page would remove if every group kept one.
    pub fn removable(&self) -> usize { self.groups.iter().map(|g| g.len().saturating_sub(1)).sum() }

    /// Bytes those files take.
    pub fn removable_bytes(&self, lib: &Library) -> u64 {
        self.groups.iter().flat_map(|g| g.iter().skip(1)).filter_map(|i| lib.photos.get(*i)).map(|p| p.size).sum()
    }

    /// The summary line.
    pub fn summary(&self, lib: &Library) -> String {
        match self.groups.len() {
            0 => "No duplicates found.".into(),
            n => format!("{n} group{} · {} file{} could go ({})", if n == 1 { "" } else { "s" },
                self.removable(), if self.removable() == 1 { "" } else { "s" },
                human_size(self.removable_bytes(lib))),
        }
    }
}

/// Draw the duplicates page.
pub fn dedup_ui(ui: &mut egui::Ui, page: &DedupPage, lib: &mut Library, textures: &mut Textures,
                c: &uikit_egui::PaletteColors) -> Option<DedupAction> {
    let mut out = None;
    ui.colored_label(c.text_weak, page.summary(lib));
    ui.colored_label(c.text_weak, egui::RichText::new(
        "Same picture by perceptual hash (5 bits of 64). Different photos of the same scene are never grouped.").small());
    if let Some(n) = &page.note { ui.colored_label(c.text, egui::RichText::new(n).small()); }
    ui.add_space(6.0);
    egui::ScrollArea::vertical().id_salt("dedup").auto_shrink([false, false]).show(ui, |ui| {
        for (gi, group) in page.groups.iter().enumerate() {
            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    for (k, &i) in group.iter().enumerate() {
                        ui.vertical(|ui| {
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(96.0, 96.0), egui::Sense::click());
                            match lib.photos.get_mut(i).and_then(|p| textures.get(ui.ctx(), p)) {
                                Some(t) => { let [w, h] = t.size();
                                    ui.painter().image(t.id(), rect, crate::grid::cover_uv(w, h), egui::Color32::WHITE); }
                                None => { ui.painter().rect_filled(rect, 3.0, c.bg_raised); }
                            }
                            if k == 0 { ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(2.0, c.success), egui::StrokeKind::Inside); }
                            if resp.clicked() { out = Some(DedupAction::Open(i)); }
                            if let Some(p) = lib.photos.get(i) {
                                ui.add(egui::Label::new(egui::RichText::new(p.name()).small().color(c.text)).truncate());
                                ui.colored_label(c.text_weak, egui::RichText::new(format!("{}×{} · {}", p.w, p.h, human_size(p.size))).small());
                            }
                            if ui.small_button(if k == 0 { "Keep this (largest)" } else { "Keep this" }).clicked() {
                                let trash: Vec<usize> = group.iter().copied().filter(|x| *x != i).collect();
                                out = Some(DedupAction::Resolve { keep: i, trash });
                            }
                        });
                    }
                });
            });
            if gi + 1 < page.groups.len() { ui.add_space(4.0); }
        }
    });
    out
}

/// What the shared page asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedAction {
    Open(usize),
    Revoke { photo: usize, contact: String },
}

/// The shared page's state: which contact is selected.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SharedPage {
    pub contact: Option<String>,
}

impl SharedPage {
    /// The rows for a contact: the library indices shared with them.
    pub fn rows(&self, share: &PhotoShare, lib: &Library) -> Vec<usize> {
        let Some(ct) = &self.contact else { return Vec::new() };
        share.visible_to(ct).into_iter().filter_map(|(k, _)| lib.find(&k)).collect()
    }
}

/// Draw the shared page.
pub fn shared_ui(ui: &mut egui::Ui, page: &mut SharedPage, share: &PhotoShare, contacts: &[Contact],
                 lib: &mut Library, textures: &mut Textures, c: &uikit_egui::PaletteColors) -> Option<SharedAction> {
    let mut out = None;
    ui.colored_label(c.text_weak, egui::RichText::new(
        "Per photo, per contact, revocable. A contact sees exactly what is shared with them. \
         The grant is kept here; the file itself travels through snapclone.").small());
    ui.add_space(6.0);
    if contacts.is_empty() {
        ui.colored_label(c.text_weak, "No snapclone contacts on this device yet — add one in snapclone and they appear here.");
        return None;
    }
    ui.horizontal_wrapped(|ui| {
        for ct in contacts {
            let n = share.visible_to(&ct.sc_id).len();
            let selected = page.contact.as_deref() == Some(ct.sc_id.as_str());
            if ui.selectable_label(selected, format!("{} ({n})", ct.label())).clicked() {
                page.contact = Some(ct.sc_id.clone());
            }
        }
    });
    ui.separator();
    let Some(ct) = page.contact.clone() else {
        ui.colored_label(c.text_weak, "Pick a contact to see what they can see.");
        return None;
    };
    let rows = page.rows(share, lib);
    if rows.is_empty() {
        ui.colored_label(c.text_weak, "Nothing shared with them. Open a photo and tap Share.");
    }
    egui::ScrollArea::vertical().id_salt("shared").auto_shrink([false, false]).show(ui, |ui| {
        for i in rows {
            ui.horizontal(|ui| {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(56.0, 56.0), egui::Sense::click());
                match lib.photos.get_mut(i).and_then(|p| textures.get(ui.ctx(), p)) {
                    Some(t) => { let [w, h] = t.size(); ui.painter().image(t.id(), rect, crate::grid::cover_uv(w, h), egui::Color32::WHITE); }
                    None => { ui.painter().rect_filled(rect, 3.0, c.bg_raised); }
                }
                if resp.clicked() { out = Some(SharedAction::Open(i)); }
                if let Some(p) = lib.photos.get(i) {
                    ui.vertical(|ui| {
                        ui.colored_label(c.text, p.name());
                        ui.colored_label(c.text_weak, egui::RichText::new(format!("{} · {}", p.day_label(), human_size(p.size))).small());
                    });
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Stop sharing").clicked() {
                        out = Some(SharedAction::Revoke { photo: i, contact: ct.clone() });
                    }
                });
            });
        }
    });
    out
}

/// What the library page asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryAction {
    /// Scan this folder instead.
    ChangeRoot(String),
    Rescan,
    /// Search for this tag on the Photos page.
    SearchTag(String),
}

/// The scan's progress, as the library page shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScanStatus {
    pub running: bool,
    pub found: usize,
    pub done: usize,
    pub from_cache: usize,
    pub failed: Vec<(String, String)>,
}

impl ScanStatus {
    pub fn start(&mut self) { *self = ScanStatus { running: true, ..Default::default() }; }

    /// One line: "scanning 40 / 312", "312 photos (300 from cache), 2 failed".
    pub fn line(&self) -> String {
        if self.running {
            if self.found == 0 { "looking for photos…".into() }
            else { format!("scanning {} / {}", self.done + self.failed.len(), self.found) }
        } else if self.found == 0 && self.done == 0 {
            "not scanned yet".into()
        } else {
            let mut s = format!("{} photo{}", self.done, if self.done == 1 { "" } else { "s" });
            if self.from_cache > 0 { s.push_str(&format!(" ({} from cache)", self.from_cache)); }
            if !self.failed.is_empty() { s.push_str(&format!(", {} failed", self.failed.len())); }
            s
        }
    }

    /// 0..=1 while running.
    pub fn fraction(&self) -> f32 {
        if self.found == 0 { 0.0 } else { ((self.done + self.failed.len()) as f32 / self.found as f32).clamp(0.0, 1.0) }
    }
}

/// The library page's state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LibraryPage {
    pub root_text: String,
    pub status: ScanStatus,
}

/// Draw the library page.
pub fn library_ui(ui: &mut egui::Ui, page: &mut LibraryPage, lib: &Library, share: &PhotoShare,
                  log: &[String], c: &uikit_egui::PaletteColors) -> Option<LibraryAction> {
    let mut out = None;
    ui.colored_label(c.text, egui::RichText::new("Folder").strong());
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut page.root_text).desired_width(260.0).hint_text("~/Pictures"));
        let entered = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let changed = page.root_text.trim() != lib.root && !page.root_text.trim().is_empty();
        if ui.add_enabled(changed && !page.status.running, egui::Button::new("Open")).clicked() || (entered && changed) {
            out = Some(LibraryAction::ChangeRoot(page.root_text.trim().to_string()));
        }
        if ui.add_enabled(!page.status.running, egui::Button::new("Rescan")).clicked() {
            out = Some(LibraryAction::Rescan);
        }
    });
    ui.colored_label(c.text_weak, egui::RichText::new(page.status.line()).small());
    if page.status.running {
        ui.add(egui::ProgressBar::new(page.status.fraction()).desired_width(260.0));
    }
    ui.add_space(8.0);
    ui.colored_label(c.text, egui::RichText::new("Library").strong());
    egui::Grid::new("lib-facts").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
        ui.colored_label(c.text_weak, "photos"); ui.colored_label(c.text, lib.len().to_string()); ui.end_row();
        ui.colored_label(c.text_weak, "on disk"); ui.colored_label(c.text, human_size(lib.total_bytes())); ui.end_row();
        ui.colored_label(c.text_weak, "shared"); ui.colored_label(c.text, format!("{} photo(s)", share.shared_count())); ui.end_row();
        ui.colored_label(c.text_weak, "index"); ui.add(egui::Label::new(egui::RichText::new(
            crate::library::Index::path_for(&lib.root).display().to_string()).small().color(c.text_weak)).truncate()); ui.end_row();
    });
    ui.add_space(8.0);
    let tags = lib.tag_counts();
    ui.colored_label(c.text, egui::RichText::new("Tags").strong());
    if tags.is_empty() {
        ui.colored_label(c.text_weak, egui::RichText::new("none written yet — open a photo to tag it").small());
    }
    ui.horizontal_wrapped(|ui| {
        for (t, n) in tags {
            if ui.small_button(format!("{t} · {n}")).clicked() { out = Some(LibraryAction::SearchTag(t)); }
        }
    });
    if !page.status.failed.is_empty() {
        ui.add_space(8.0);
        ui.colored_label(c.warning, egui::RichText::new(format!("{} file(s) could not be read", page.status.failed.len())).strong());
        for (p, why) in page.status.failed.iter().take(20) {
            ui.add(egui::Label::new(egui::RichText::new(format!("{p}: {why}")).small().color(c.text_weak)).truncate());
        }
    }
    ui.add_space(8.0);
    ui.collapsing("Log", |ui| {
        for line in log.iter().rev().take(30) {
            ui.add(egui::Label::new(egui::RichText::new(line).small().monospace().color(c.text_weak)).truncate());
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::tests::sample;

    #[test]
    fn the_dedup_summary_counts_what_would_go_and_how_much_it_weighs() {
        let mut lib = sample();
        let mut d = DedupPage::default();
        d.refresh(&lib);
        assert_eq!(d.groups.len(), 1);
        assert_eq!(d.removable(), 1);
        assert!(d.summary(&lib).starts_with("1 group · 1 file could go"), "{}", d.summary(&lib));
        lib.remove("/pics/beach-copy.jpg");
        d.refresh(&lib);
        assert_eq!(d.summary(&lib), "No duplicates found.");
    }

    #[test]
    fn the_shared_page_lists_a_contacts_photos_by_library_index() {
        let lib = sample();
        let mut share = PhotoShare::new("me");
        let rec = crate::share::PhotoRecord { name: "beach.jpg".into(), size: 1, dhash: 1, tags: vec![], at: 0 };
        share.share_with("sc-ada", "/pics/beach.jpg", &rec);
        share.share_with("sc-ada", "/pics/gone.jpg", &rec);
        let page = SharedPage { contact: Some("sc-ada".into()) };
        let rows = page.rows(&share, &lib);
        assert_eq!(rows, vec![lib.find("/pics/beach.jpg").unwrap()], "a photo not in the library is not a row");
        assert!(SharedPage::default().rows(&share, &lib).is_empty());
    }

    #[test]
    fn the_scan_line_tells_the_story_from_start_to_finish() {
        let mut s = ScanStatus::default();
        assert_eq!(s.line(), "not scanned yet");
        s.start();
        assert_eq!(s.line(), "looking for photos…");
        s.found = 10; s.done = 3; s.failed.push(("x".into(), "bad".into()));
        assert_eq!(s.line(), "scanning 4 / 10");
        assert!((s.fraction() - 0.4).abs() < 1e-6);
        s.running = false; s.done = 9; s.from_cache = 5;
        assert_eq!(s.line(), "9 photos (5 from cache), 1 failed");
    }
}
//! pages — Duplicates, Shared and Library.
//!
//! Each is a function over the shell's data that returns what was asked
//! for. The decisions worth testing (which of a duplicate group to keep,
//! what the shared page lists for a contact, what the library page says
//! about a scan) are on the pure types; the functions draw them.

use crate::library::Library;
use crate::share::{Contact, PhotoShare};
use crate::textures::Textures;
use crate::viewer::human_size;

/// What the duplicates page asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DedupAction {
    /// Keep `keep`, trash the rest of its group.
    Resolve { keep: usize, trash: Vec<usize> },
    Open(usize),
}

/// The duplicate groups, cached between library changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DedupPage {
    pub groups: Vec<Vec<usize>>,
    /// True when the library changed since `groups` was computed.
    pub stale: bool,
    pub note: Option<String>,
}

impl DedupPage {
    pub fn refresh(&mut self, lib: &Library) {
        self.groups = lib.duplicate_groups();
        self.stale = false;
    }

    /// How many files the page would remove if every group kept one.
    pub fn removable(&self) -> usize { self.groups.iter().map(|g| g.len().saturating_sub(1)).sum() }

    /// Bytes those files take.
    pub fn removable_bytes(&self, lib: &Library) -> u64 {
        self.groups.iter().flat_map(|g| g.iter().skip(1)).filter_map(|i| lib.photos.get(*i)).map(|p| p.size).sum()
    }

    /// The summary line.
    pub fn summary(&self, lib: &Library) -> String {
        match self.groups.len() {
            0 => "No duplicates found.".into(),
            n => format!("{n} group{} · {} file{} could go ({})", if n == 1 { "" } else { "s" },
                self.removable(), if self.removable() == 1 { "" } else { "s" },
                human_size(self.removable_bytes(lib))),
        }
    }
}

/// Draw the duplicates page.
pub fn dedup_ui(ui: &mut egui::Ui, page: &DedupPage, lib: &mut Library, textures: &mut Textures,
                c: &uikit_egui::PaletteColors) -> Option<DedupAction> {
    let mut out = None;
    ui.colored_label(c.text_weak, page.summary(lib));
    ui.colored_label(c.text_weak, egui::RichText::new(
        "Same picture by perceptual hash (5 bits of 64). Different photos of the same scene are never grouped.").small());
    if let Some(n) = &page.note { ui.colored_label(c.text, egui::RichText::new(n).small()); }
    ui.add_space(6.0);
    egui::ScrollArea::vertical().id_salt("dedup").auto_shrink([false, false]).show(ui, |ui| {
        for (gi, group) in page.groups.iter().enumerate() {
            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    for (k, &i) in group.iter().enumerate() {
                        ui.vertical(|ui| {
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(96.0, 96.0), egui::Sense::click());
                            match lib.photos.get_mut(i).and_then(|p| textures.get(ui.ctx(), p)) {
                                Some(t) => { let [w, h] = t.size();
                                    ui.painter().image(t.id(), rect, crate::grid::cover_uv(w, h), egui::Color32::WHITE); }
                                None => { ui.painter().rect_filled(rect, 3.0, c.bg_raised); }
                            }
                            if k == 0 { ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(2.0, c.success), egui::StrokeKind::Inside); }
                            if resp.clicked() { out = Some(DedupAction::Open(i)); }
                            if let Some(p) = lib.photos.get(i) {
                                ui.add(egui::Label::new(egui::RichText::new(p.name()).small().color(c.text)).truncate());
                                ui.colored_label(c.text_weak, egui::RichText::new(format!("{}×{} · {}", p.w, p.h, human_size(p.size))).small());
                            }
                            if ui.small_button(if k == 0 { "Keep this (largest)" } else { "Keep this" }).clicked() {
                                let trash: Vec<usize> = group.iter().copied().filter(|x| *x != i).collect();
                                out = Some(DedupAction::Resolve { keep: i, trash });
                            }
                        });
                    }
                });
            });
            if gi + 1 < page.groups.len() { ui.add_space(4.0); }
        }
    });
    out
}

/// What the shared page asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedAction {
    Open(usize),
    Revoke { photo: usize, contact: String },
}

/// The shared page's state: which contact is selected.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SharedPage {
    pub contact: Option<String>,
}

impl SharedPage {
    /// The rows for a contact: the library indices shared with them.
    pub fn rows(&self, share: &PhotoShare, lib: &Library) -> Vec<usize> {
        let Some(ct) = &self.contact else { return Vec::new() };
        share.visible_to(ct).into_iter().filter_map(|(k, _)| lib.find(&k)).collect()
    }
}

/// Draw the shared page.
pub fn shared_ui(ui: &mut egui::Ui, page: &mut SharedPage, share: &PhotoShare, contacts: &[Contact],
                 lib: &mut Library, textures: &mut Textures, c: &uikit_egui::PaletteColors) -> Option<SharedAction> {
    let mut out = None;
    ui.colored_label(c.text_weak, egui::RichText::new(
        "Per photo, per contact, revocable. A contact sees exactly what is shared with them. \
         The grant is kept here; the file itself travels through snapclone.").small());
    ui.add_space(6.0);
    if contacts.is_empty() {
        ui.colored_label(c.text_weak, "No snapclone contacts on this device yet — add one in snapclone and they appear here.");
        return None;
    }
    ui.horizontal_wrapped(|ui| {
        for ct in contacts {
            let n = share.visible_to(&ct.sc_id).len();
            let selected = page.contact.as_deref() == Some(ct.sc_id.as_str());
            if ui.selectable_label(selected, format!("{} ({n})", ct.label())).clicked() {
                page.contact = Some(ct.sc_id.clone());
            }
        }
    });
    ui.separator();
    let Some(ct) = page.contact.clone() else {
        ui.colored_label(c.text_weak, "Pick a contact to see what they can see.");
        return None;
    };
    let rows = page.rows(share, lib);
    if rows.is_empty() {
        ui.colored_label(c.text_weak, "Nothing shared with them. Open a photo and tap Share.");
    }
    egui::ScrollArea::vertical().id_salt("shared").auto_shrink([false, false]).show(ui, |ui| {
        for i in rows {
            ui.horizontal(|ui| {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(56.0, 56.0), egui::Sense::click());
                match lib.photos.get_mut(i).and_then(|p| textures.get(ui.ctx(), p)) {
                    Some(t) => { let [w, h] = t.size(); ui.painter().image(t.id(), rect, crate::grid::cover_uv(w, h), egui::Color32::WHITE); }
                    None => { ui.painter().rect_filled(rect, 3.0, c.bg_raised); }
                }
                if resp.clicked() { out = Some(SharedAction::Open(i)); }
                if let Some(p) = lib.photos.get(i) {
                    ui.vertical(|ui| {
                        ui.colored_label(c.text, p.name());
                        ui.colored_label(c.text_weak, egui::RichText::new(format!("{} · {}", p.day_label(), human_size(p.size))).small());
                    });
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Stop sharing").clicked() {
                        out = Some(SharedAction::Revoke { photo: i, contact: ct.clone() });
                    }
                });
            });
        }
    });
    out
}

/// What the library page asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryAction {
    /// Scan this folder instead.
    ChangeRoot(String),
    Rescan,
    /// Search for this tag on the Photos page.
    SearchTag(String),
}

/// The scan's progress, as the library page shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScanStatus {
    pub running: bool,
    pub found: usize,
    pub done: usize,
    pub from_cache: usize,
    pub failed: Vec<(String, String)>,
}

impl ScanStatus {
    pub fn start(&mut self) { *self = ScanStatus { running: true, ..Default::default() }; }

    /// One line: "scanning 40 / 312", "312 photos (300 from cache), 2 failed".
    pub fn line(&self) -> String {
        if self.running {
            if self.found == 0 { "looking for photos…".into() }
            else { format!("scanning {} / {}", self.done + self.failed.len(), self.found) }
        } else if self.found == 0 && self.done == 0 {
            "not scanned yet".into()
        } else {
            let mut s = format!("{} photo{}", self.done, if self.done == 1 { "" } else { "s" });
            if self.from_cache > 0 { s.push_str(&format!(" ({} from cache)", self.from_cache)); }
            if !self.failed.is_empty() { s.push_str(&format!(", {} failed", self.failed.len())); }
            s
        }
    }

    /// 0..=1 while running.
    pub fn fraction(&self) -> f32 {
        if self.found == 0 { 0.0 } else { ((self.done + self.failed.len()) as f32 / self.found as f32).clamp(0.0, 1.0) }
    }
}

/// The library page's state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LibraryPage {
    pub root_text: String,
    pub status: ScanStatus,
}

/// Draw the library page.
pub fn library_ui(ui: &mut egui::Ui, page: &mut LibraryPage, lib: &Library, share: &PhotoShare,
                  log: &[String], c: &uikit_egui::PaletteColors) -> Option<LibraryAction> {
    let mut out = None;
    ui.colored_label(c.text, egui::RichText::new("Folder").strong());
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut page.root_text).desired_width(260.0).hint_text("~/Pictures"));
        let entered = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let changed = page.root_text.trim() != lib.root && !page.root_text.trim().is_empty();
        if ui.add_enabled(changed && !page.status.running, egui::Button::new("Open")).clicked() || (entered && changed) {
            out = Some(LibraryAction::ChangeRoot(page.root_text.trim().to_string()));
        }
        if ui.add_enabled(!page.status.running, egui::Button::new("Rescan")).clicked() {
            out = Some(LibraryAction::Rescan);
        }
    });
    ui.colored_label(c.text_weak, egui::RichText::new(page.status.line()).small());
    if page.status.running {
        ui.add(egui::ProgressBar::new(page.status.fraction()).desired_width(260.0));
    }
    ui.add_space(8.0);
    ui.colored_label(c.text, egui::RichText::new("Library").strong());
    egui::Grid::new("lib-facts").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
        ui.colored_label(c.text_weak, "photos"); ui.colored_label(c.text, lib.len().to_string()); ui.end_row();
        ui.colored_label(c.text_weak, "on disk"); ui.colored_label(c.text, human_size(lib.total_bytes())); ui.end_row();
        ui.colored_label(c.text_weak, "shared"); ui.colored_label(c.text, format!("{} photo(s)", share.shared_count())); ui.end_row();
        ui.colored_label(c.text_weak, "index"); ui.add(egui::Label::new(egui::RichText::new(
            crate::library::Index::path_for(&lib.root).display().to_string()).small().color(c.text_weak)).truncate()); ui.end_row();
    });
    ui.add_space(8.0);
    let tags = lib.tag_counts();
    ui.colored_label(c.text, egui::RichText::new("Tags").strong());
    if tags.is_empty() {
        ui.colored_label(c.text_weak, egui::RichText::new("none written yet — open a photo to tag it").small());
    }
    ui.horizontal_wrapped(|ui| {
        for (t, n) in tags {
            if ui.small_button(format!("{t} · {n}")).clicked() { out = Some(LibraryAction::SearchTag(t)); }
        }
    });
    if !page.status.failed.is_empty() {
        ui.add_space(8.0);
        ui.colored_label(c.warning, egui::RichText::new(format!("{} file(s) could not be read", page.status.failed.len())).strong());
        for (p, why) in page.status.failed.iter().take(20) {
            ui.add(egui::Label::new(egui::RichText::new(format!("{p}: {why}")).small().color(c.text_weak)).truncate());
        }
    }
    ui.add_space(8.0);
    ui.collapsing("Log", |ui| {
        for line in log.iter().rev().take(30) {
            ui.add(egui::Label::new(egui::RichText::new(line).small().monospace().color(c.text_weak)).truncate());
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::tests::sample;

    #[test]
    fn the_dedup_summary_counts_what_would_go_and_how_much_it_weighs() {
        let mut lib = sample();
        let mut d = DedupPage::default();
        d.refresh(&lib);
        assert_eq!(d.groups.len(), 1);
        assert_eq!(d.removable(), 1);
        assert!(d.summary(&lib).starts_with("1 group · 1 file could go"), "{}", d.summary(&lib));
        lib.remove("/pics/beach-copy.jpg");
        d.refresh(&lib);
        assert_eq!(d.summary(&lib), "No duplicates found.");
    }

    #[test]
    fn the_shared_page_lists_a_contacts_photos_by_library_index() {
        let lib = sample();
        let mut share = PhotoShare::new("me");
        let rec = crate::share::PhotoRecord { name: "beach.jpg".into(), size: 1, dhash: 1, tags: vec![], at: 0 };
        share.share_with("sc-ada", "/pics/beach.jpg", &rec);
        share.share_with("sc-ada", "/pics/gone.jpg", &rec);
        let page = SharedPage { contact: Some("sc-ada".into()) };
        let rows = page.rows(&share, &lib);
        assert_eq!(rows, vec![lib.find("/pics/beach.jpg").unwrap()], "a photo not in the library is not a row");
        assert!(SharedPage::default().rows(&share, &lib).is_empty());
    }

    #[test]
    fn the_scan_line_tells_the_story_from_start_to_finish() {
        let mut s = ScanStatus::default();
        assert_eq!(s.line(), "not scanned yet");
        s.start();
        assert_eq!(s.line(), "looking for photos…");
        s.found = 10; s.done = 3; s.failed.push(("x".into(), "bad".into()));
        assert_eq!(s.line(), "scanning 4 / 10");
        assert!((s.fraction() - 0.4).abs() < 1e-6);
        s.running = false; s.done = 9; s.from_cache = 5;
        assert_eq!(s.line(), "9 photos (5 from cache), 1 failed");
    }
}
