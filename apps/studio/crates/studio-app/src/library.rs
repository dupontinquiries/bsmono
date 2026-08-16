//! library — the sound-library sidebar.
//!
//! A folder of samples IS the instrument for a sample-based workflow; making
//! the app the place you browse it removes the file-manager round trip that
//! drag-and-drop still requires. The sidebar lists what `STUDIO_LIBRARY`
//! (default `./library`) holds and offers each file the same two doors a
//! drop already has: a rack sampler channel (a drum hit) or a timeline clip
//! at the playhead (a melody loop). One scan function, pure and tested; the
//! egui around it stays dumb.
//!
//! Audition-on-click (hear a sample before committing it) needs a live
//! one-shot voice over the sink's buffer — named future work with the
//! playing-mode keys, not faked by hijacking the loop buffer.

use std::path::{Path, PathBuf};

/// Extensions studio-media can decode. The sniffing decoder would accept
/// more, but a browser that lists files it cannot open teaches mistrust.
pub const AUDIO_EXTS: &[&str] = &["opus", "wav", "flac", "ogg", "mp3", "m4a", "aac"];

/// One listed sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    /// Shown name: path relative to the library root, so `kicks/808.opus`
    /// reads as the folder structure the user built.
    pub label: String,
    pub path: PathBuf,
}

/// Scan `root` (two levels: the root and its immediate subfolders — a
/// library is folders-of-kits, and a full recursive walk of a mistyped
/// `STUDIO_LIBRARY=/` must not hang the app) for decodable audio, sorted by
/// label so kits group together.
pub fn scan(root: &Path) -> Vec<LibraryEntry> {
    fn is_audio(p: &Path) -> bool {
        p.extension().and_then(|e| e.to_str())
            .map(|e| AUDIO_EXTS.iter().any(|a| e.eq_ignore_ascii_case(a)))
            .unwrap_or(false)
    }
    let mut out: Vec<PathBuf> = Vec::new();
    let Ok(top) = std::fs::read_dir(root) else { return Vec::new() };
    for entry in top.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Ok(sub) = std::fs::read_dir(&p) {
                for e in sub.flatten() {
                    let q = e.path();
                    if q.is_file() && is_audio(&q) {
                        out.push(q);
                    }
                }
            }
        } else if is_audio(&p) {
            out.push(p);
        }
    }
    let mut entries: Vec<LibraryEntry> = out.into_iter()
        .map(|p| LibraryEntry {
            label: p.strip_prefix(root).unwrap_or(&p).display().to_string(),
            path: p,
        })
        .collect();
    entries.sort_by(|a, b| a.label.cmp(&b.label));
    entries
}

/// What the sidebar asked the shell to do with a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryAction {
    ToRack(PathBuf),
    ToTimeline(PathBuf),
}

/// Draw the sidebar body. Returns at most one action — a person clicks one
/// button per frame, and returning edits keeps this as mutation-free as the
/// grid.
pub fn show(
    ui: &mut egui::Ui,
    root: &Path,
    entries: &[LibraryEntry],
    c: &uikit_egui::PaletteColors,
) -> Option<LibraryAction> {
    let mut action = None;
    ui.label(format!("library — {}", root.display()));
    if entries.is_empty() {
        ui.colored_label(c.text_weak,
            "empty. Set STUDIO_LIBRARY or put samples in ./library");
        return None;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for e in entries {
            ui.horizontal(|ui| {
                ui.label(&e.label);
                if ui.small_button("rack").on_hover_text("add as a sampler channel").clicked() {
                    action = Some(LibraryAction::ToRack(e.path.clone()));
                }
                if ui.small_button("clip").on_hover_text("place at the playhead").clicked() {
                    action = Some(LibraryAction::ToTimeline(e.path.clone()));
                }
            });
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scan_lists_audio_sorted_and_nothing_else() {
        let root = std::env::temp_dir().join(format!("studio-lib-{}", std::process::id()));
        let kicks = root.join("kicks");
        std::fs::create_dir_all(&kicks).unwrap();
        std::fs::write(root.join("melody.opus"), b"x").unwrap();
        std::fs::write(root.join("notes.txt"), b"x").unwrap();
        std::fs::write(kicks.join("808.WAV"), b"x").unwrap(); // case-insensitive
        std::fs::write(kicks.join("readme.md"), b"x").unwrap();
        let deep = kicks.join("deeper");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("too-deep.wav"), b"x").unwrap(); // 3rd level: out

        let got = scan(&root);
        let labels: Vec<_> = got.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, vec!["kicks/808.WAV", "melody.opus"],
                   "audio only, two levels, sorted so kits group");
    }

    #[test]
    fn a_missing_library_is_an_empty_list_not_a_crash() {
        assert!(scan(Path::new("/definitely/not/a/real/library")).is_empty());
    }
}
