//! Archive — fao_app's video library ("+ New Archive", "Open Archive", "+ Add Video", a list
//! whose rows open in the Player), ported over the same `fao_codec_lib::Archive` calls.
//!
//! An archive is a folder with an `archive.fao_lib` manifest listing .fao videos with their
//! size, rate, length, chapters and tags. Every change is saved at once, as in the GTK app.
//! Divergences, recorded: native file choosers become path fields (as on the Library page);
//! an entry's metadata is read from the first 4 KB of the .fao (its 30-byte header) and the
//! file's size from the filesystem, where fao_app read the whole file into memory; rows whose
//! file is gone are marked instead of failing silently when opened; "Remove" (which the
//! library has and fao_app never offered) drops the entry, never the file.
//!
//! Not a different name for the Library page: that one is the FAO Web Library, a
//! canonical-mesh catalog shared across encodes. This is a list of videos.

use fao_codec_lib as fao_v1;
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "archive.fao_lib";

#[derive(Default)]
pub struct State {
    /// Folder (New) or manifest file (Open), typed.
    pub path: String,
    /// A .fao to add, typed.
    pub add_path: String,
    pub archive: Option<fao_v1::Archive>,
    pub manifest: Option<PathBuf>,
    pub status: String,
}

/// An archive entry for a .fao file: its header's size, rate and frame count, the file's size.
pub fn entry_for(fao: &Path, source: &str) -> Result<fao_v1::VideoEntry, String> {
    use std::io::Read;
    let mut head = Vec::with_capacity(4096);
    std::fs::File::open(fao).and_then(|f| f.take(4096).read_to_end(&mut head)).map_err(|e| format!("{}: {e}", fao.display()))?;
    let h = fao_v1::read_fao_header(&head).ok_or_else(|| format!("{}: not a .fao file", fao.display()))?;
    let size = std::fs::metadata(fao).map(|m| m.len()).unwrap_or(0);
    let fps = h.fps_num as f32 / h.fps_den.max(1) as f32;
    Ok(fao_v1::VideoEntry {
        id: 0, // assigned by add_video
        title: fao.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into()),
        source_path: source.to_string(),
        fao_path: fao.display().to_string(),
        width: h.width as u32,
        height: h.height as u32,
        fps,
        frame_count: h.frame_count,
        duration_secs: h.frame_count as f32 / fps.max(1.0),
        file_size_bytes: size,
        created_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        chapters: Vec::new(),
        tags: Vec::new(),
        thumbnail_data: Vec::new(),
    })
}

impl State {
    pub fn is_open(&self) -> bool { self.archive.is_some() }

    fn save(&mut self) -> Result<(), String> {
        match (&self.archive, &self.manifest) {
            (Some(a), Some(m)) => a.save_manifest(m).map_err(|e| format!("saving {}: {e}", m.display())),
            _ => Err("no archive open".into()),
        }
    }

    /// "+ New Archive": `folder/archive.fao_lib`, named after the folder. Refuses to replace an
    /// existing manifest (open it instead).
    pub fn create(&mut self, folder: &Path) -> Result<(), String> {
        let m = folder.join(MANIFEST);
        if m.exists() { return Err(format!("{} already exists: open it instead", m.display())); }
        std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
        self.archive = Some(fao_v1::Archive::new(name.clone(), folder.display().to_string()));
        self.manifest = Some(m);
        self.save()?;
        self.status = format!("created archive {name}");
        Ok(())
    }

    /// "Open Archive": a manifest file, or a folder holding one.
    pub fn open(&mut self, path: &Path) -> Result<(), String> {
        let m = if path.is_dir() { path.join(MANIFEST) } else { path.to_path_buf() };
        let a = fao_v1::Archive::load_manifest(&m).map_err(|e| format!("{}: {e}", m.display()))?;
        self.status = format!("opened {} ({} videos)", a.name, a.entries.len());
        self.archive = Some(a);
        self.manifest = Some(m);
        Ok(())
    }

    /// "+ Add Video", and the Encode page's automatic add. A file already listed is not added
    /// twice.
    pub fn add(&mut self, fao: &Path, source: &str) -> Result<u32, String> {
        let entry = entry_for(fao, source)?;
        let a = self.archive.as_mut().ok_or("no archive open")?;
        if let Some(e) = a.entries.iter().find(|e| Path::new(&e.fao_path) == fao) {
            return Err(format!("{} is already in the archive", e.title));
        }
        a.add_video(entry);
        let id = a.entries.last().map(|e| e.id).unwrap_or(0);
        self.save()?;
        self.status = format!("added {}", fao.display());
        Ok(id)
    }

    /// Drop an entry from the list; the .fao file itself is left alone.
    pub fn remove(&mut self, id: u32) -> Result<(), String> {
        let a = self.archive.as_mut().ok_or("no archive open")?;
        if !a.remove_video(id) { return Err(format!("no entry {id}")); }
        self.save()?;
        self.status = "removed from the list (file kept)".into();
        Ok(())
    }
}

fn mb(n: u64) -> String { format!("{:.1} MB", n as f64 / 1_048_576.0) }

pub fn ui(app: &mut crate::App, ui: &mut egui::Ui) {
    ui.heading("Archive");
    ui.label(egui::RichText::new("A folder of .fao videos with an archive.fao_lib manifest. Encodes finished on the Encode page are added to the open archive.").weak().small());
    ui.add_space(6.0);
    let s = &mut app.archive;
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut s.path).id_salt("archive-path").hint_text("folder (new) or archive.fao_lib (open)").desired_width(340.0));
        if ui.button("+ New Archive").clicked() {
            let p = PathBuf::from(s.path.trim());
            if let Err(e) = s.create(&p) { s.status = e; }
        }
        if ui.button("Open Archive").clicked() {
            let p = PathBuf::from(s.path.trim());
            if let Err(e) = s.open(&p) { s.status = e; }
        }
    });
    if !s.status.is_empty() { ui.label(egui::RichText::new(&s.status).small()); }
    let Some(a) = &s.archive else { return };
    ui.add_space(6.0);
    ui.label(egui::RichText::new(format!("{} — {} videos, {:.0} s, {}", a.name, a.entries.len(), a.total_duration_secs(), mb(a.total_size_bytes))).strong());
    if let Some(m) = &s.manifest { ui.label(egui::RichText::new(m.display().to_string()).monospace().small().weak()); }
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut s.add_path).id_salt("archive-add").hint_text("/path/to/video.fao").desired_width(340.0));
        if ui.button("+ Add Video").clicked() {
            let p = PathBuf::from(s.add_path.trim());
            if let Err(e) = s.add(&p, "") { s.status = e; }
        }
    });
    ui.separator();
    let mut open: Option<String> = None;
    let mut remove: Option<u32> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let Some(a) = &s.archive else { return };
        if a.entries.is_empty() { ui.label(egui::RichText::new("(no videos yet)").weak()); }
        for e in &a.entries {
            let exists = Path::new(&e.fao_path).is_file();
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&e.title).strong());
                    ui.label(egui::RichText::new(format!("{}×{} {:.1} fps  {:.1} s  {}  {} chapters{}", e.width, e.height, e.fps, e.duration_secs, mb(e.file_size_bytes), e.chapters.len(),
                        if e.tags.is_empty() { String::new() } else { format!("  [{}]", e.tags.join(", ")) })).small());
                    if !exists { ui.colored_label(ui.visuals().warn_fg_color, format!("missing: {}", e.fao_path)); }
                });
                if ui.add_enabled(exists, egui::Button::new("▶ Open")).clicked() { open = Some(e.fao_path.clone()); }
                if ui.button("Remove").clicked() { remove = Some(e.id); }
            });
            ui.separator();
        }
    });
    if let Some(id) = remove {
        if let Err(e) = app.archive.remove(id) { app.archive.status = e; }
    }
    if let Some(p) = open {
        app.player.path = p;
        app.player.open();
        app.tab = crate::Tab::Player;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcode::{EncodeOpts, FrameEncoder};

    /// A real .fao written by the same encoder the Encode page uses (no ffmpeg needed).
    fn make_fao(path: &Path, frames: usize) {
        let (w, h) = (64, 48);
        let mut enc = FrameEncoder::new(w, h, EncodeOpts::inter(70));
        let cf: Vec<_> = (0..frames).map(|f| {
            let rgb: Vec<u8> = (0..w * h * 3).map(|i| ((i / 3 + f * 5) % 256) as u8).collect();
            let e = enc.encode(&rgb);
            (Vec::new(), e.payload.clone(), e.stat.marker, e.payload.len(), 0.0f64, 0.0f64)
        }).collect();
        let mut out = Vec::new();
        fao_v1::write_fao_container(&mut out, w, h, 25.0, 0.7, &cf).unwrap();
        std::fs::write(path, out).unwrap();
    }

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fao-archive-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn create_add_reopen_remove() {
        let d = dir("life");
        let v = d.join("clip.fao");
        make_fao(&v, 10);
        let mut s = State::default();
        s.create(&d.join("lib")).unwrap();
        assert!(s.create(&d.join("lib")).unwrap_err().contains("already exists"), "never replaces a manifest");
        let id = s.add(&v, "/src/clip.mp4").unwrap();
        assert!(s.add(&v, "").unwrap_err().contains("already in the archive"));
        // Reopened from disk (by folder), the entry carries the header's numbers.
        let mut t = State::default();
        t.open(&d.join("lib")).unwrap();
        let a = t.archive.as_ref().unwrap();
        assert_eq!((a.name.as_str(), a.entries.len()), ("lib", 1));
        let e = &a.entries[0];
        assert_eq!((e.title.as_str(), e.width, e.height, e.frame_count, e.source_path.as_str()), ("clip", 64, 48, 10, "/src/clip.mp4"));
        assert!((e.fps - 25.0).abs() < 0.01 && (e.duration_secs - 0.4).abs() < 0.01, "{} fps {} s", e.fps, e.duration_secs);
        assert_eq!(e.file_size_bytes, std::fs::metadata(&v).unwrap().len());
        t.remove(id).unwrap();
        assert!(v.is_file(), "remove keeps the file");
        let mut u = State::default();
        u.open(&d.join("lib").join(MANIFEST)).unwrap();
        assert!(u.archive.unwrap().entries.is_empty(), "removal was saved");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn not_a_fao_and_no_archive_are_errors_with_reasons() {
        let d = dir("bad");
        let txt = d.join("notes.fao");
        std::fs::write(&txt, "hello").unwrap();
        let mut s = State::default();
        assert!(s.add(&txt, "").is_err());
        s.create(&d).unwrap();
        assert!(s.add(&txt, "").unwrap_err().contains("not a .fao"));
        assert!(State::default().open(&d.join("nothing.fao_lib")).is_err());
        std::fs::write(d.join("broken.fao_lib"), b"FAOLIB01\x05\x00\x00\x00ab").unwrap();
        assert!(State::default().open(&d.join("broken.fao_lib")).unwrap_err().contains("truncated"), "a damaged manifest says so");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_finished_encode_lands_in_the_open_archive_and_opens_in_the_player() {
        let d = dir("encode");
        let v = d.join("enc.fao");
        make_fao(&v, 6);
        let mut app = crate::App::default();
        app.archive.create(&d).unwrap();
        app.encode.input = "/src/enc.mp4".into();
        app.encode.finished = Some(v.clone());
        crate::encode_page::add_finished_to_archive(&mut app);
        assert!(app.encode.archive_note.contains("added to archive"), "{}", app.encode.archive_note);
        let e = &app.archive.archive.as_ref().unwrap().entries[0];
        assert_eq!((e.fao_path.as_str(), e.source_path.as_str()), (v.to_str().unwrap(), "/src/enc.mp4"));
        // With no archive open, nothing happens and nothing is said.
        let mut app2 = crate::App::default();
        app2.encode.finished = Some(v.clone());
        crate::encode_page::add_finished_to_archive(&mut app2);
        assert!(app2.encode.archive_note.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    #[allow(deprecated)]
    fn every_nav_glyph_renders_in_the_bundled_fonts() {
        // Asked of the running font stack (uikit icons), not assumed: a tab label in tofu is
        // the c11 field report again.
        let ctx = egui::Context::default();
        let mut missing = vec![];
        // Frame 1 installs the app's fonts (as App::ui does); egui applies them from the next
        // frame, so the probe runs on frame 2.
        for frame in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                uikit_egui::fonts::install(ctx);
                if frame == 1 {
                    for c in ['⏺', '🎞', '⏱', '🔗', '▶', '🗄', '⚙'] {
                        // epaint's glyph table is the authority; uikit's metric probe can
                        // report a real glyph as tofu when their advances coincide (its doc
                        // says so). Record both so a disagreement is visible.
                        let has = ctx.fonts_mut(|f| f.has_glyph(&egui::FontId::proportional(14.0), c));
                        let probe = uikit_egui::icons::renders(ctx, c);
                        eprintln!("{c} U+{:04X}: has_glyph {has}, uikit probe {probe}", c as u32);
                        if !has { missing.push(c); }
                    }
                }
            });
        }
        assert!(missing.is_empty(), "tofu in the nav: {missing:?}");
    }
}

