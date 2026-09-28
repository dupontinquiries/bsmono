//! photosync-egui — a photo library you can look through, clean up, tag
//! and share, on the headless `photosync` core.
//!
//! ## Layers
//!
//! * `photosync` (the parent crate): pixels in, hashes/ascii/tags out. No
//!   decoder, no disk, no window. Untouched here except for one fix
//!   (`tag_similarity` now applies the auto-tag weight it documented).
//! * [`library`], [`share`]: what the app knows — photos, days, tags,
//!   duplicate groups, grants — as plain data with pure queries.
//! * [`scan`], [`textures`]: the two places bytes move — files decoded on
//!   a worker thread, thumbnails uploaded to the GPU with a ceiling.
//! * [`grid`], [`viewer`], [`pages`]: the drawing, each returning what
//!   was pressed.
//! * [`Shell`]: owns the state, dispatches to the pages, applies what
//!   they return, and queues the few things that touch the disk for the
//!   host (`bin/photosync_egui.rs`), which is the only file with a window.
//!
//! ## What it deliberately does not do
//!
//! No face detection (the core's `cv` doc says why: a heuristic one
//! mislabels a library invisibly). No EXIF — dates are file dates and the
//! UI says "modified", not "taken". No byte transfer to a contact — the
//! grant table is here, the transport is snapclone's, and the Shared page
//! says so instead of showing a "sent" that did not happen.

pub mod grid;
pub mod library;
pub mod pages;
pub mod scan;
pub mod share;
pub mod textures;
pub mod viewer;

use library::{DayGroup, Library, Photo};
use pages::{DedupAction, DedupPage, LibraryAction, LibraryPage, ScanStatus, SharedAction, SharedPage};
use share::{Contact, PhotoRecord, PhotoShare};
use textures::Textures;
use viewer::{Viewer, ViewerAction};

/// The four pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section { Photos, Duplicates, Shared, Library }

impl Section {
    pub const ALL: [Section; 4] = [Section::Photos, Section::Duplicates, Section::Shared, Section::Library];

    pub fn label(self) -> &'static str {
        match self {
            Section::Photos => "Photos",
            Section::Duplicates => "Duplicates",
            Section::Shared => "Shared",
            Section::Library => "Library",
        }
    }
}

/// What the shell cannot do itself: everything that touches the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostAction {
    /// Move these files to the library's trash folder.
    Trash(Vec<String>),
    /// Scan the current root again.
    Rescan,
    /// Scan a different folder.
    ChangeRoot(String),
    /// The index or the grants changed; write them.
    Save,
}

/// The "similar to" strip: the query photo and its matches with scores.
#[derive(Debug, Clone, PartialEq)]
pub struct Similar {
    pub to: usize,
    pub matches: Vec<(usize, f32)>,
}

/// The app's state and its dispatch.
pub struct Shell {
    pub section: Section,
    pub library: Library,
    pub textures: Textures,
    pub query: String,
    pub viewer: Option<Viewer>,
    pub similar: Option<Similar>,
    pub dedup: DedupPage,
    pub shared: SharedPage,
    pub lib_page: LibraryPage,
    pub share: PhotoShare,
    pub contacts: Vec<Contact>,
    pub log: Vec<String>,
    pub pending: Vec<HostAction>,
    /// The days for the current query, regrouped when either changes.
    days: Vec<DayGroup>,
    days_for: (String, u64),
    /// Bumped on every library change.
    generation: u64,
}

impl Shell {
    pub fn new(cache_dir: std::path::PathBuf, owner: &str) -> Shell {
        Shell {
            section: Section::Photos,
            library: Library::default(),
            textures: Textures::new(cache_dir),
            query: String::new(),
            viewer: None,
            similar: None,
            dedup: DedupPage::default(),
            shared: SharedPage::default(),
            lib_page: LibraryPage::default(),
            share: PhotoShare::new(owner),
            contacts: Vec::new(),
            log: Vec::new(),
            pending: Vec::new(),
            days: Vec::new(),
            days_for: (String::new(), u64::MAX),
            generation: 0,
        }
    }

    pub fn note(&mut self, s: impl Into<String>) {
        let s = s.into();
        self.log.push(s);
        if self.log.len() > 200 { self.log.drain(..self.log.len() - 200); }
    }

    fn touched(&mut self) {
        self.generation += 1;
        self.dedup.stale = true;
    }

    /// A photo from the scanner.
    pub fn add_photo(&mut self, p: Photo) {
        // Written tags on an existing entry survive a rescan; the scanner
        // carried them through the index, but a photo tagged during THIS
        // scan is only in memory.
        if let Some(i) = self.library.find(&p.path) {
            let mut p = p;
            for t in self.library.photos[i].written_tags() {
                let t = t.to_string();
                if !p.entry.tags.contains(&t) { p.entry.tags.push(t); }
            }
            p.entry.tags.sort();
            self.library.photos[i] = p;
        } else {
            self.library.upsert(p);
        }
        self.touched();
    }

    /// The days the grid shows, for the current query.
    pub fn days(&mut self) -> &[DayGroup] {
        let key = (self.query.clone(), self.generation);
        if self.days_for != key {
            self.days = self.library.days(&self.query);
            self.days_for = key;
        }
        &self.days
    }

    /// Open the viewer on a library index, paging through the grid's
    /// current order (or the similar strip when it came from there).
    pub fn open_photo(&mut self, i: usize) {
        let seq: Vec<usize> = if let Some(s) = self.similar.as_ref().filter(|s| s.matches.iter().any(|(j, _)| *j == i)) {
            s.matches.iter().map(|(j, _)| *j).collect()
        } else {
            self.days().iter().flat_map(|d| d.photos.iter().copied()).collect()
        };
        let pos = seq.iter().position(|j| *j == i).unwrap_or(0);
        let seq = if seq.is_empty() { vec![i] } else { seq };
        self.viewer = Some(Viewer::open(seq, pos));
    }

    /// Show the photos most like this one.
    pub fn find_similar(&mut self, i: usize) {
        let matches = self.library.like(i, 24);
        let name = self.library.photos.get(i).map(|p| p.name().to_string()).unwrap_or_default();
        self.note(format!("{} photo(s) like {name}", matches.len()));
        self.similar = Some(Similar { to: i, matches });
        self.section = Section::Photos;
        self.viewer = None;
    }

    fn record_for(&self, i: usize) -> Option<PhotoRecord> {
        let p = self.library.photos.get(i)?;
        Some(PhotoRecord {
            name: p.name().to_string(), size: p.size, dhash: p.entry.dhash,
            tags: p.written_tags().iter().map(|t| t.to_string()).collect(), at: p.at,
        })
    }

    /// Remove photos from the library and ask the host to trash the files.
    pub fn trash(&mut self, mut indices: Vec<usize>) {
        indices.sort_unstable();
        indices.dedup();
        let mut paths = Vec::new();
        for &i in indices.iter().rev() {
            if let Some(p) = self.library.photos.get(i).map(|p| p.path.clone()) {
                self.textures.drop_photo(&p);
                self.library.remove(&p);
                if let Some(v) = &mut self.viewer {
                    if !v.remove_index(i) { self.viewer = None; }
                }
                if let Some(s) = &mut self.similar {
                    s.matches.retain(|(j, _)| *j != i);
                    for (j, _) in &mut s.matches { if *j > i { *j -= 1; } }
                    if s.to == i { self.similar = None; } else if s.to > i { s.to -= 1; }
                }
                paths.push(p);
            }
        }
        if !paths.is_empty() {
            self.note(format!("trashing {} file(s)", paths.len()));
            self.pending.push(HostAction::Trash(paths));
            self.pending.push(HostAction::Save);
            self.touched();
        }
    }

    pub fn apply_viewer(&mut self, a: ViewerAction) {
        match a {
            ViewerAction::Close => self.viewer = None,
            ViewerAction::AddTag(i, t) => {
                if self.library.add_tag(i, &t) { self.touched(); self.pending.push(HostAction::Save); }
            }
            ViewerAction::RemoveTag(i, t) => {
                if self.library.remove_tag(i, &t) { self.touched(); self.pending.push(HostAction::Save); }
            }
            ViewerAction::Share(i, contact) => {
                if let (Some(rec), Some(p)) = (self.record_for(i), self.library.photos.get(i)) {
                    let key = p.path.clone();
                    self.share.share_with(&contact, &key, &rec);
                    self.note(format!("shared {} with {contact}", rec.name));
                    self.pending.push(HostAction::Save);
                }
            }
            ViewerAction::Revoke(i, contact) => {
                if let Some(p) = self.library.photos.get(i) {
                    let key = p.path.clone();
                    self.share.revoke(&contact, &key);
                    self.note(format!("stopped sharing {} with {contact}", p.name()));
                    self.pending.push(HostAction::Save);
                }
            }
            ViewerAction::Like(i) => self.find_similar(i),
            ViewerAction::Trash(i) => self.trash(vec![i]),
        }
    }

    /// Draw everything.
    pub fn ui(&mut self, ui: &mut egui::Ui, c: &uikit_egui::PaletteColors) {
        self.textures.begin_frame();
        let compact = uikit_egui::adaptive::Adaptive::detect(ui.ctx()).class.is_compact();
        if self.viewer.is_some() {
            let mut v = self.viewer.take().unwrap();
            let acts = viewer::viewer_ui(ui, &mut v, &mut self.library, &mut self.textures,
                                        &self.share, &self.contacts, compact, c);
            self.viewer = Some(v);
            for a in acts { self.apply_viewer(a); }
            self.textures.evict(textures::CAP);
            return;
        }
        // Duplicate groups are recomputed when the library settles, so the
        // tab can carry its count before anyone opens it.
        if self.dedup.stale && !self.lib_page.status.running { self.dedup.refresh(&self.library); }
        ui.horizontal(|ui| {
            for s in Section::ALL {
                let label = match s {
                    Section::Duplicates if !self.dedup.groups.is_empty() => format!("Duplicates ({})", self.dedup.groups.len()),
                    _ => s.label().to_string(),
                };
                if ui.selectable_label(self.section == s, label).clicked() { self.section = s; }
            }
        });
        ui.separator();
        match self.section {
            Section::Photos => self.photos_ui(ui, compact, c),
            Section::Duplicates => {
                if self.dedup.stale { self.dedup.refresh(&self.library); }
                if let Some(a) = pages::dedup_ui(ui, &self.dedup.clone(), &mut self.library, &mut self.textures, c) {
                    match a {
                        DedupAction::Open(i) => self.open_photo(i),
                        DedupAction::Resolve { keep, trash } => {
                            let name = self.library.photos.get(keep).map(|p| p.name().to_string()).unwrap_or_default();
                            self.dedup.note = Some(format!("kept {name}, trashed {}", trash.len()));
                            self.trash(trash);
                        }
                    }
                }
            }
            Section::Shared => {
                let mut page = std::mem::take(&mut self.shared);
                let a = pages::shared_ui(ui, &mut page, &self.share, &self.contacts, &mut self.library, &mut self.textures, c);
                self.shared = page;
                match a {
                    Some(SharedAction::Open(i)) => self.open_photo(i),
                    Some(SharedAction::Revoke { photo, contact }) => self.apply_viewer(ViewerAction::Revoke(photo, contact)),
                    None => {}
                }
            }
            Section::Library => {
                let mut page = std::mem::take(&mut self.lib_page);
                let a = pages::library_ui(ui, &mut page, &self.library, &self.share, &self.log, c);
                self.lib_page = page;
                match a {
                    Some(LibraryAction::ChangeRoot(r)) => self.pending.push(HostAction::ChangeRoot(r)),
                    Some(LibraryAction::Rescan) => self.pending.push(HostAction::Rescan),
                    Some(LibraryAction::SearchTag(t)) => { self.query = t; self.similar = None; self.section = Section::Photos; }
                    None => {}
                }
            }
        }
        self.textures.evict(textures::CAP);
    }

    fn photos_ui(&mut self, ui: &mut egui::Ui, compact: bool, c: &uikit_egui::PaletteColors) {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("search names and tags").desired_width(if compact { 180.0 } else { 260.0 }));
            if !self.query.is_empty() && ui.small_button("✕").clicked() { self.query.clear(); }
            let status = &self.lib_page.status;
            if status.running {
                ui.colored_label(c.text_weak, egui::RichText::new(status.line()).small());
            } else {
                ui.colored_label(c.text_weak, egui::RichText::new(format!("{} photos", self.library.len())).small());
            }
        });
        // The similar strip, above the days.
        let mut open = None;
        if let Some(s) = self.similar.clone() {
            ui.horizontal(|ui| {
                let name = self.library.photos.get(s.to).map(|p| p.name().to_string()).unwrap_or_default();
                ui.colored_label(c.text, egui::RichText::new(format!("Similar to {name}")).strong());
                if ui.small_button("✕").clicked() { self.similar = None; }
            });
            egui::ScrollArea::horizontal().id_salt("similar").show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, score) in &s.matches {
                        ui.vertical(|ui| {
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(88.0, 88.0), egui::Sense::click());
                            match self.library.photos.get_mut(*i).and_then(|p| self.textures.get(ui.ctx(), p)) {
                                Some(t) => { let [w, h] = t.size(); ui.painter().image(t.id(), rect, grid::cover_uv(w, h), egui::Color32::WHITE); }
                                None => { ui.painter().rect_filled(rect, 3.0, c.bg_raised); }
                            }
                            if resp.clicked() { open = Some(*i); }
                            ui.colored_label(c.text_weak, egui::RichText::new(format!("{:.0}%", score * 100.0)).small());
                        });
                    }
                    if s.matches.is_empty() { ui.colored_label(c.text_weak, "nothing else is like it"); }
                });
            });
            ui.separator();
        }
        let days = self.days().to_vec();
        if let Some(grid::GridAction::Open(i)) = grid::grid_ui(ui, &mut self.library, &days, &mut self.textures, compact, c) {
            open = Some(i);
        }
        if let Some(i) = open { self.open_photo(i); }
    }

    /// The scan status, for the host to update.
    pub fn scan_status(&mut self) -> &mut ScanStatus { &mut self.lib_page.status }
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::tests::sample;

    fn shell() -> Shell {
        let mut s = Shell::new(std::env::temp_dir().join("photosync-test-cache"), "sc-me");
        for p in sample().photos { s.add_photo(p); }
        s.library.root = "/pics".into();
        s
    }

    #[test]
    fn opening_from_the_grid_pages_in_grid_order_and_from_similar_in_similar_order() {
        let mut s = shell();
        let beach = s.library.find("/pics/beach.jpg").unwrap();
        s.open_photo(beach);
        let v = s.viewer.as_ref().unwrap();
        assert_eq!(v.seq.len(), 4);
        assert_eq!(v.current(), Some(beach));
        assert_eq!(v.position_text(), "2 / 4", "second newest in the grid");
        s.find_similar(beach);
        assert!(s.viewer.is_none(), "finding similar returns to the grid");
        let first = s.similar.as_ref().unwrap().matches[0].0;
        s.open_photo(first);
        assert_eq!(s.viewer.as_ref().unwrap().seq.len(), 3, "paging through the matches, not the grid");
    }

    #[test]
    fn tagging_marks_the_index_dirty_and_a_search_regroups() {
        let mut s = shell();
        let i = s.library.find("/pics/bright.png").unwrap();
        s.apply_viewer(ViewerAction::AddTag(i, "Sunset".into()));
        assert_eq!(s.pending, vec![HostAction::Save]);
        s.query = "sunset".into();
        assert_eq!(s.days().len(), 1);
        assert_eq!(s.days()[0].photos, vec![i]);
    }

    #[test]
    fn sharing_records_a_grant_and_the_shared_page_sees_it() {
        let mut s = shell();
        let i = s.library.find("/pics/beach.jpg").unwrap();
        s.apply_viewer(ViewerAction::Share(i, "sc-ada".into()));
        assert!(s.share.is_shared_with("/pics/beach.jpg", "sc-ada"));
        s.shared.contact = Some("sc-ada".into());
        assert_eq!(s.shared.rows(&s.share, &s.library), vec![i]);
        s.apply_viewer(ViewerAction::Revoke(i, "sc-ada".into()));
        assert!(s.shared.rows(&s.share, &s.library).is_empty());
    }

    #[test]
    fn trashing_removes_from_the_library_the_viewer_and_the_similar_strip_and_asks_the_host() {
        let mut s = shell();
        let beach = s.library.find("/pics/beach.jpg").unwrap();
        s.find_similar(beach);
        let copy = s.library.find("/pics/beach-copy.jpg").unwrap();
        s.open_photo(copy);
        s.trash(vec![copy]);
        assert!(s.library.find("/pics/beach-copy.jpg").is_none());
        assert!(s.pending.contains(&HostAction::Trash(vec!["/pics/beach-copy.jpg".into()])));
        assert!(s.similar.as_ref().unwrap().matches.iter().all(|(j, _)| s.library.photos.get(*j).is_some()),
                "the strip's indices still point at real photos");
        assert!(s.viewer.as_ref().map(|v| v.seq.iter().all(|j| *j < s.library.len())).unwrap_or(true));
        // Duplicates are recomputed lazily after a change.
        assert!(s.dedup.stale);
        s.dedup.refresh(&s.library);
        assert!(s.dedup.groups.is_empty());
    }

    fn run(shell: &mut Shell, size: egui::Vec2) -> (uikit_egui::layout_probe::Report, Vec<uikit_egui::layout_probe::Text>) {
        let colors = uikit_egui::palette_colors("cloudscape");
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(screen);
        let mut last = Vec::new();
        for _ in 0..3 {
            #[allow(deprecated)]
            let out = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { shell.ui(ui, &colors); });
            });
            last = out.shapes;
        }
        let compact = size.x < 600.0;
        (uikit_egui::layout_probe::analyse(screen, &last, compact), uikit_egui::layout_probe::texts(&last))
    }

    #[test]
    fn every_page_lays_out_without_collisions_on_a_phone_and_a_desktop() {
        for section in Section::ALL {
            for size in [egui::vec2(360.0, 640.0), egui::vec2(1000.0, 700.0)] {
                let mut s = shell();
                s.contacts = vec![share::Contact { sc_id: "sc-ada".into(), nickname: "Ada".into() }];
                s.apply_viewer(ViewerAction::Share(0, "sc-ada".into()));
                s.shared.contact = Some("sc-ada".into());
                s.section = section;
                let (r, texts) = run(&mut s, size);
                assert!(r.overlaps.is_empty(), "{section:?} at {size:?}: {}", r.describe());
                assert!(!texts.is_empty(), "{section:?} drew nothing");
            }
        }
    }

    #[test]
    fn the_viewer_with_its_panel_lays_out_on_a_phone_and_beside_the_picture_on_a_desktop() {
        for size in [egui::vec2(360.0, 640.0), egui::vec2(1000.0, 700.0)] {
            let mut s = shell();
            s.open_photo(0);
            s.viewer.as_mut().unwrap().info = true;
            let (r, texts) = run(&mut s, size);
            assert!(r.overlaps.is_empty(), "viewer at {size:?}: {}", r.describe());
            let tags = texts.iter().find(|t| t.text == "Tags").expect("the panel is drawn");
            if size.x > 600.0 {
                assert!(tags.rect.left() > size.x * 0.5, "the panel is beside the picture: {:?}", tags.rect);
            } else {
                assert!(tags.rect.top() > size.y * 0.4, "the panel is under the picture: {:?}", tags.rect);
            }
        }
    }

    #[test]
    fn a_rescanned_photo_keeps_the_tags_written_this_session() {
        let mut s = shell();
        let i = s.library.find("/pics/dark.jpg").unwrap();
        s.library.add_tag(i, "cellar");
        let fresh = library::tests::photo("/pics/dark.jpg", 99, &vec![8u8; 32 * 32], 32, 32, &[]);
        s.add_photo(fresh);
        let i = s.library.find("/pics/dark.jpg").unwrap();
        assert!(s.library.photos[i].written_tags().contains(&"cellar"));
        assert_eq!(s.library.photos[i].at, 99, "and is otherwise the fresh scan");
    }
}
//! photosync-egui — a photo library you can look through, clean up, tag
//! and share, on the headless `photosync` core.
//!
//! ## Layers
//!
//! * `photosync` (the parent crate): pixels in, hashes/ascii/tags out. No
//!   decoder, no disk, no window. Untouched here except for one fix
//!   (`tag_similarity` now applies the auto-tag weight it documented).
//! * [`library`], [`share`]: what the app knows — photos, days, tags,
//!   duplicate groups, grants — as plain data with pure queries.
//! * [`scan`], [`textures`]: the two places bytes move — files decoded on
//!   a worker thread, thumbnails uploaded to the GPU with a ceiling.
//! * [`grid`], [`viewer`], [`pages`]: the drawing, each returning what
//!   was pressed.
//! * [`Shell`]: owns the state, dispatches to the pages, applies what
//!   they return, and queues the few things that touch the disk for the
//!   host (`bin/photosync_egui.rs`), which is the only file with a window.
//!
//! ## What it deliberately does not do
//!
//! No face detection (the core's `cv` doc says why: a heuristic one
//! mislabels a library invisibly). No EXIF — dates are file dates and the
//! UI says "modified", not "taken". No byte transfer to a contact — the
//! grant table is here, the transport is snapclone's, and the Shared page
//! says so instead of showing a "sent" that did not happen.

pub mod grid;
pub mod library;
pub mod pages;
pub mod scan;
pub mod share;
pub mod textures;
pub mod viewer;

use library::{DayGroup, Library, Photo};
use pages::{DedupAction, DedupPage, LibraryAction, LibraryPage, ScanStatus, SharedAction, SharedPage};
use share::{Contact, PhotoRecord, PhotoShare};
use textures::Textures;
use viewer::{Viewer, ViewerAction};

/// The four pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section { Photos, Duplicates, Shared, Library }

impl Section {
    pub const ALL: [Section; 4] = [Section::Photos, Section::Duplicates, Section::Shared, Section::Library];

    pub fn label(self) -> &'static str {
        match self {
            Section::Photos => "Photos",
            Section::Duplicates => "Duplicates",
            Section::Shared => "Shared",
            Section::Library => "Library",
        }
    }
}

/// What the shell cannot do itself: everything that touches the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostAction {
    /// Move these files to the library's trash folder.
    Trash(Vec<String>),
    /// Scan the current root again.
    Rescan,
    /// Scan a different folder.
    ChangeRoot(String),
    /// The index or the grants changed; write them.
    Save,
}

/// The "similar to" strip: the query photo and its matches with scores.
#[derive(Debug, Clone, PartialEq)]
pub struct Similar {
    pub to: usize,
    pub matches: Vec<(usize, f32)>,
}

/// The app's state and its dispatch.
pub struct Shell {
    pub section: Section,
    pub library: Library,
    pub textures: Textures,
    pub query: String,
    pub viewer: Option<Viewer>,
    pub similar: Option<Similar>,
    pub dedup: DedupPage,
    pub shared: SharedPage,
    pub lib_page: LibraryPage,
    pub share: PhotoShare,
    pub contacts: Vec<Contact>,
    pub log: Vec<String>,
    pub pending: Vec<HostAction>,
    /// The days for the current query, regrouped when either changes.
    days: Vec<DayGroup>,
    days_for: (String, u64),
    /// Bumped on every library change.
    generation: u64,
}

impl Shell {
    pub fn new(cache_dir: std::path::PathBuf, owner: &str) -> Shell {
        Shell {
            section: Section::Photos,
            library: Library::default(),
            textures: Textures::new(cache_dir),
            query: String::new(),
            viewer: None,
            similar: None,
            dedup: DedupPage::default(),
            shared: SharedPage::default(),
            lib_page: LibraryPage::default(),
            share: PhotoShare::new(owner),
            contacts: Vec::new(),
            log: Vec::new(),
            pending: Vec::new(),
            days: Vec::new(),
            days_for: (String::new(), u64::MAX),
            generation: 0,
        }
    }

    pub fn note(&mut self, s: impl Into<String>) {
        let s = s.into();
        self.log.push(s);
        if self.log.len() > 200 { self.log.drain(..self.log.len() - 200); }
    }

    fn touched(&mut self) {
        self.generation += 1;
        self.dedup.stale = true;
    }

    /// A photo from the scanner.
    pub fn add_photo(&mut self, p: Photo) {
        // Written tags on an existing entry survive a rescan; the scanner
        // carried them through the index, but a photo tagged during THIS
        // scan is only in memory.
        if let Some(i) = self.library.find(&p.path) {
            let mut p = p;
            for t in self.library.photos[i].written_tags() {
                let t = t.to_string();
                if !p.entry.tags.contains(&t) { p.entry.tags.push(t); }
            }
            p.entry.tags.sort();
            self.library.photos[i] = p;
        } else {
            self.library.upsert(p);
        }
        self.touched();
    }

    /// The days the grid shows, for the current query.
    pub fn days(&mut self) -> &[DayGroup] {
        let key = (self.query.clone(), self.generation);
        if self.days_for != key {
            self.days = self.library.days(&self.query);
            self.days_for = key;
        }
        &self.days
    }

    /// Open the viewer on a library index, paging through the grid's
    /// current order (or the similar strip when it came from there).
    pub fn open_photo(&mut self, i: usize) {
        let seq: Vec<usize> = if let Some(s) = self.similar.as_ref().filter(|s| s.matches.iter().any(|(j, _)| *j == i)) {
            s.matches.iter().map(|(j, _)| *j).collect()
        } else {
            self.days().iter().flat_map(|d| d.photos.iter().copied()).collect()
        };
        let pos = seq.iter().position(|j| *j == i).unwrap_or(0);
        let seq = if seq.is_empty() { vec![i] } else { seq };
        self.viewer = Some(Viewer::open(seq, pos));
    }

    /// Show the photos most like this one.
    pub fn find_similar(&mut self, i: usize) {
        let matches = self.library.like(i, 24);
        let name = self.library.photos.get(i).map(|p| p.name().to_string()).unwrap_or_default();
        self.note(format!("{} photo(s) like {name}", matches.len()));
        self.similar = Some(Similar { to: i, matches });
        self.section = Section::Photos;
        self.viewer = None;
    }

    fn record_for(&self, i: usize) -> Option<PhotoRecord> {
        let p = self.library.photos.get(i)?;
        Some(PhotoRecord {
            name: p.name().to_string(), size: p.size, dhash: p.entry.dhash,
            tags: p.written_tags().iter().map(|t| t.to_string()).collect(), at: p.at,
        })
    }

    /// Remove photos from the library and ask the host to trash the files.
    pub fn trash(&mut self, mut indices: Vec<usize>) {
        indices.sort_unstable();
        indices.dedup();
        let mut paths = Vec::new();
        for &i in indices.iter().rev() {
            if let Some(p) = self.library.photos.get(i).map(|p| p.path.clone()) {
                self.textures.drop_photo(&p);
                self.library.remove(&p);
                if let Some(v) = &mut self.viewer {
                    if !v.remove_index(i) { self.viewer = None; }
                }
                if let Some(s) = &mut self.similar {
                    s.matches.retain(|(j, _)| *j != i);
                    for (j, _) in &mut s.matches { if *j > i { *j -= 1; } }
                    if s.to == i { self.similar = None; } else if s.to > i { s.to -= 1; }
                }
                paths.push(p);
            }
        }
        if !paths.is_empty() {
            self.note(format!("trashing {} file(s)", paths.len()));
            self.pending.push(HostAction::Trash(paths));
            self.pending.push(HostAction::Save);
            self.touched();
        }
    }

    pub fn apply_viewer(&mut self, a: ViewerAction) {
        match a {
            ViewerAction::Close => self.viewer = None,
            ViewerAction::AddTag(i, t) => {
                if self.library.add_tag(i, &t) { self.touched(); self.pending.push(HostAction::Save); }
            }
            ViewerAction::RemoveTag(i, t) => {
                if self.library.remove_tag(i, &t) { self.touched(); self.pending.push(HostAction::Save); }
            }
            ViewerAction::Share(i, contact) => {
                if let (Some(rec), Some(p)) = (self.record_for(i), self.library.photos.get(i)) {
                    let key = p.path.clone();
                    self.share.share_with(&contact, &key, &rec);
                    self.note(format!("shared {} with {contact}", rec.name));
                    self.pending.push(HostAction::Save);
                }
            }
            ViewerAction::Revoke(i, contact) => {
                if let Some(p) = self.library.photos.get(i) {
                    let key = p.path.clone();
                    self.share.revoke(&contact, &key);
                    self.note(format!("stopped sharing {} with {contact}", p.name()));
                    self.pending.push(HostAction::Save);
                }
            }
            ViewerAction::Like(i) => self.find_similar(i),
            ViewerAction::Trash(i) => self.trash(vec![i]),
        }
    }

    /// Draw everything.
    pub fn ui(&mut self, ui: &mut egui::Ui, c: &uikit_egui::PaletteColors) {
        self.textures.begin_frame();
        let compact = uikit_egui::adaptive::Adaptive::detect(ui.ctx()).class.is_compact();
        if self.viewer.is_some() {
            let mut v = self.viewer.take().unwrap();
            let acts = viewer::viewer_ui(ui, &mut v, &mut self.library, &mut self.textures,
                                        &self.share, &self.contacts, compact, c);
            self.viewer = Some(v);
            for a in acts { self.apply_viewer(a); }
            self.textures.evict(textures::CAP);
            return;
        }
        // Duplicate groups are recomputed when the library settles, so the
        // tab can carry its count before anyone opens it.
        if self.dedup.stale && !self.lib_page.status.running { self.dedup.refresh(&self.library); }
        ui.horizontal(|ui| {
            for s in Section::ALL {
                let label = match s {
                    Section::Duplicates if !self.dedup.groups.is_empty() => format!("Duplicates ({})", self.dedup.groups.len()),
                    _ => s.label().to_string(),
                };
                if ui.selectable_label(self.section == s, label).clicked() { self.section = s; }
            }
        });
        ui.separator();
        match self.section {
            Section::Photos => self.photos_ui(ui, compact, c),
            Section::Duplicates => {
                if self.dedup.stale { self.dedup.refresh(&self.library); }
                if let Some(a) = pages::dedup_ui(ui, &self.dedup.clone(), &mut self.library, &mut self.textures, c) {
                    match a {
                        DedupAction::Open(i) => self.open_photo(i),
                        DedupAction::Resolve { keep, trash } => {
                            let name = self.library.photos.get(keep).map(|p| p.name().to_string()).unwrap_or_default();
                            self.dedup.note = Some(format!("kept {name}, trashed {}", trash.len()));
                            self.trash(trash);
                        }
                    }
                }
            }
            Section::Shared => {
                let mut page = std::mem::take(&mut self.shared);
                let a = pages::shared_ui(ui, &mut page, &self.share, &self.contacts, &mut self.library, &mut self.textures, c);
                self.shared = page;
                match a {
                    Some(SharedAction::Open(i)) => self.open_photo(i),
                    Some(SharedAction::Revoke { photo, contact }) => self.apply_viewer(ViewerAction::Revoke(photo, contact)),
                    None => {}
                }
            }
            Section::Library => {
                let mut page = std::mem::take(&mut self.lib_page);
                let a = pages::library_ui(ui, &mut page, &self.library, &self.share, &self.log, c);
                self.lib_page = page;
                match a {
                    Some(LibraryAction::ChangeRoot(r)) => self.pending.push(HostAction::ChangeRoot(r)),
                    Some(LibraryAction::Rescan) => self.pending.push(HostAction::Rescan),
                    Some(LibraryAction::SearchTag(t)) => { self.query = t; self.similar = None; self.section = Section::Photos; }
                    None => {}
                }
            }
        }
        self.textures.evict(textures::CAP);
    }

    fn photos_ui(&mut self, ui: &mut egui::Ui, compact: bool, c: &uikit_egui::PaletteColors) {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("search names and tags").desired_width(if compact { 180.0 } else { 260.0 }));
            if !self.query.is_empty() && ui.small_button("✕").clicked() { self.query.clear(); }
            let status = &self.lib_page.status;
            if status.running {
                ui.colored_label(c.text_weak, egui::RichText::new(status.line()).small());
            } else {
                ui.colored_label(c.text_weak, egui::RichText::new(format!("{} photos", self.library.len())).small());
            }
        });
        // The similar strip, above the days.
        let mut open = None;
        if let Some(s) = self.similar.clone() {
            ui.horizontal(|ui| {
                let name = self.library.photos.get(s.to).map(|p| p.name().to_string()).unwrap_or_default();
                ui.colored_label(c.text, egui::RichText::new(format!("Similar to {name}")).strong());
                if ui.small_button("✕").clicked() { self.similar = None; }
            });
            egui::ScrollArea::horizontal().id_salt("similar").show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, score) in &s.matches {
                        ui.vertical(|ui| {
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(88.0, 88.0), egui::Sense::click());
                            match self.library.photos.get_mut(*i).and_then(|p| self.textures.get(ui.ctx(), p)) {
                                Some(t) => { let [w, h] = t.size(); ui.painter().image(t.id(), rect, grid::cover_uv(w, h), egui::Color32::WHITE); }
                                None => { ui.painter().rect_filled(rect, 3.0, c.bg_raised); }
                            }
                            if resp.clicked() { open = Some(*i); }
                            ui.colored_label(c.text_weak, egui::RichText::new(format!("{:.0}%", score * 100.0)).small());
                        });
                    }
                    if s.matches.is_empty() { ui.colored_label(c.text_weak, "nothing else is like it"); }
                });
            });
            ui.separator();
        }
        let days = self.days().to_vec();
        if let Some(grid::GridAction::Open(i)) = grid::grid_ui(ui, &mut self.library, &days, &mut self.textures, compact, c) {
            open = Some(i);
        }
        if let Some(i) = open { self.open_photo(i); }
    }

    /// The scan status, for the host to update.
    pub fn scan_status(&mut self) -> &mut ScanStatus { &mut self.lib_page.status }
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::tests::sample;

    fn shell() -> Shell {
        let mut s = Shell::new(std::env::temp_dir().join("photosync-test-cache"), "sc-me");
        for p in sample().photos { s.add_photo(p); }
        s.library.root = "/pics".into();
        s
    }

    #[test]
    fn opening_from_the_grid_pages_in_grid_order_and_from_similar_in_similar_order() {
        let mut s = shell();
        let beach = s.library.find("/pics/beach.jpg").unwrap();
        s.open_photo(beach);
        let v = s.viewer.as_ref().unwrap();
        assert_eq!(v.seq.len(), 4);
        assert_eq!(v.current(), Some(beach));
        assert_eq!(v.position_text(), "2 / 4", "second newest in the grid");
        s.find_similar(beach);
        assert!(s.viewer.is_none(), "finding similar returns to the grid");
        let first = s.similar.as_ref().unwrap().matches[0].0;
        s.open_photo(first);
        assert_eq!(s.viewer.as_ref().unwrap().seq.len(), 3, "paging through the matches, not the grid");
    }

    #[test]
    fn tagging_marks_the_index_dirty_and_a_search_regroups() {
        let mut s = shell();
        let i = s.library.find("/pics/bright.png").unwrap();
        s.apply_viewer(ViewerAction::AddTag(i, "Sunset".into()));
        assert_eq!(s.pending, vec![HostAction::Save]);
        s.query = "sunset".into();
        assert_eq!(s.days().len(), 1);
        assert_eq!(s.days()[0].photos, vec![i]);
    }

    #[test]
    fn sharing_records_a_grant_and_the_shared_page_sees_it() {
        let mut s = shell();
        let i = s.library.find("/pics/beach.jpg").unwrap();
        s.apply_viewer(ViewerAction::Share(i, "sc-ada".into()));
        assert!(s.share.is_shared_with("/pics/beach.jpg", "sc-ada"));
        s.shared.contact = Some("sc-ada".into());
        assert_eq!(s.shared.rows(&s.share, &s.library), vec![i]);
        s.apply_viewer(ViewerAction::Revoke(i, "sc-ada".into()));
        assert!(s.shared.rows(&s.share, &s.library).is_empty());
    }

    #[test]
    fn trashing_removes_from_the_library_the_viewer_and_the_similar_strip_and_asks_the_host() {
        let mut s = shell();
        let beach = s.library.find("/pics/beach.jpg").unwrap();
        s.find_similar(beach);
        let copy = s.library.find("/pics/beach-copy.jpg").unwrap();
        s.open_photo(copy);
        s.trash(vec![copy]);
        assert!(s.library.find("/pics/beach-copy.jpg").is_none());
        assert!(s.pending.contains(&HostAction::Trash(vec!["/pics/beach-copy.jpg".into()])));
        assert!(s.similar.as_ref().unwrap().matches.iter().all(|(j, _)| s.library.photos.get(*j).is_some()),
                "the strip's indices still point at real photos");
        assert!(s.viewer.as_ref().map(|v| v.seq.iter().all(|j| *j < s.library.len())).unwrap_or(true));
        // Duplicates are recomputed lazily after a change.
        assert!(s.dedup.stale);
        s.dedup.refresh(&s.library);
        assert!(s.dedup.groups.is_empty());
    }

    fn run(shell: &mut Shell, size: egui::Vec2) -> (uikit_egui::layout_probe::Report, Vec<uikit_egui::layout_probe::Text>) {
        let colors = uikit_egui::palette_colors("cloudscape");
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(screen);
        let mut last = Vec::new();
        for _ in 0..3 {
            #[allow(deprecated)]
            let out = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { shell.ui(ui, &colors); });
            });
            last = out.shapes;
        }
        let compact = size.x < 600.0;
        (uikit_egui::layout_probe::analyse(screen, &last, compact), uikit_egui::layout_probe::texts(&last))
    }

    #[test]
    fn every_page_lays_out_without_collisions_on_a_phone_and_a_desktop() {
        for section in Section::ALL {
            for size in [egui::vec2(360.0, 640.0), egui::vec2(1000.0, 700.0)] {
                let mut s = shell();
                s.contacts = vec![share::Contact { sc_id: "sc-ada".into(), nickname: "Ada".into() }];
                s.apply_viewer(ViewerAction::Share(0, "sc-ada".into()));
                s.shared.contact = Some("sc-ada".into());
                s.section = section;
                let (r, texts) = run(&mut s, size);
                assert!(r.overlaps.is_empty(), "{section:?} at {size:?}: {}", r.describe());
                assert!(!texts.is_empty(), "{section:?} drew nothing");
            }
        }
    }

    #[test]
    fn the_viewer_with_its_panel_lays_out_on_a_phone_and_beside_the_picture_on_a_desktop() {
        for size in [egui::vec2(360.0, 640.0), egui::vec2(1000.0, 700.0)] {
            let mut s = shell();
            s.open_photo(0);
            s.viewer.as_mut().unwrap().info = true;
            let (r, texts) = run(&mut s, size);
            assert!(r.overlaps.is_empty(), "viewer at {size:?}: {}", r.describe());
            let tags = texts.iter().find(|t| t.text == "Tags").expect("the panel is drawn");
            if size.x > 600.0 {
                assert!(tags.rect.left() > size.x * 0.5, "the panel is beside the picture: {:?}", tags.rect);
            } else {
                assert!(tags.rect.top() > size.y * 0.4, "the panel is under the picture: {:?}", tags.rect);
            }
        }
    }

    #[test]
    fn a_rescanned_photo_keeps_the_tags_written_this_session() {
        let mut s = shell();
        let i = s.library.find("/pics/dark.jpg").unwrap();
        s.library.add_tag(i, "cellar");
        let fresh = library::tests::photo("/pics/dark.jpg", 99, &vec![8u8; 32 * 32], 32, 32, &[]);
        s.add_photo(fresh);
        let i = s.library.find("/pics/dark.jpg").unwrap();
        assert!(s.library.photos[i].written_tags().contains(&"cellar"));
        assert_eq!(s.library.photos[i].at, 99, "and is otherwise the fresh scan");
    }
}
