//! The file picker.
//!
//! Directory navigation is `uikit_egui::path_picker` — its `PathPicker` owns
//! the stack, the up/into history and the hidden-files toggle, and
//! `list_local` does the read. What it does NOT do is files: `Listing` carries
//! `dirs` only, because it was written to choose a folder. So this module adds
//! the file half and nothing else, rather than reimplementing navigation
//! alongside it.
//!
//! The pure parts — which extensions count, how the list is filtered and
//! sorted — are here and tested. A picker that quietly hides the file the user
//! is looking for is worse than one that fails to open.

use uikit_egui::fuzzy;

/// Extensions the demuxers can actually open.
///
/// Deliberately short. Listing `.mkv` or `.avi` because they are video would
/// be a lie: nothing here parses them, and a file that appears in the picker
/// and then fails to open is a worse experience than one that never appeared.
/// `.mkv` IS Matroska and the WebM demuxer would very likely read it, but that
/// is a claim to test rather than assume, so it stays out until it is.
pub const PLAYABLE: &[&str] = &["mp4", "m4v", "mov", "webm"];

/// Does this filename look playable?
pub fn is_playable(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((_, ext)) => PLAYABLE.iter().any(|p| ext.eq_ignore_ascii_case(p)),
        None => false,
    }
}

/// One row in the picker.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

/// Read a directory into directories and playable files.
///
/// Unreadable entries are skipped rather than failing the whole listing: one
/// permission-denied file in a directory should not make the directory
/// unbrowsable.
pub fn read_dir(path: &std::path::Path, show_hidden: bool) -> Result<Vec<Entry>, String> {
    let rd = std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir || is_playable(&name) {
            out.push(Entry { name, is_dir });
        }
    }
    Ok(sort_entries(out))
}

/// Directories first, then files, each case-insensitively alphabetical.
///
/// Directories first because the picker is used to navigate more often than to
/// choose, and interleaving them makes both jobs harder.
pub fn sort_entries(mut v: Vec<Entry>) -> Vec<Entry> {
    v.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    v
}

/// Narrow a listing with the same fuzzy matcher the palette uses.
///
/// Ranked rather than merely filtered, so typing a few characters puts the
/// intended file at the top instead of somewhere in a filtered list that is
/// still too long to scan.
pub fn filter(entries: &[Entry], query: &str) -> Vec<Entry> {
    if query.trim().is_empty() {
        return entries.to_vec();
    }
    let mut scored: Vec<(i32, bool, Entry)> = entries
        .iter()
        .filter_map(|e| fuzzy::score(query, &e.name).map(|s| (s, e.is_dir, e.clone())))
        .collect();
    // Directories still lead at equal score, so filtering does not silently
    // reorder the two kinds relative to each other.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    scored.into_iter().map(|(_, _, e)| e).collect()
}

/// Where the picker should start.
///
/// The directory of the file already open, so reopening lands where the user
/// was; otherwise the home directory, and the working directory only if home
/// is unknown. Starting at `/` would be technically correct and useless.
pub fn start_dir(current: Option<&std::path::Path>) -> std::path::PathBuf {
    if let Some(p) = current {
        if let Some(parent) = p.parent() {
            if parent.is_dir() {
                return parent.to_path_buf();
            }
        }
    }
    std::env::var("HOME")
        .ok()
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, is_dir: bool) -> Entry {
        Entry {
            name: name.into(),
            is_dir,
        }
    }

    #[test]
    fn known_containers_are_playable_and_others_are_not() {
        for ok in ["clip.mp4", "a.webm", "B.MP4", "x.MOV", "y.m4v"] {
            assert!(is_playable(ok), "{ok} should be playable");
        }
        for no in ["notes.txt", "clip.mkv", "image.png", "mp4", "", "archive.mp4.gz"] {
            assert!(!is_playable(no), "{no} should not be playable");
        }
    }

    /// A file that appears and then fails to open is worse than one that never
    /// appeared, so the list must not promise formats nothing parses.
    #[test]
    fn the_playable_list_only_names_formats_a_demuxer_handles() {
        for ext in PLAYABLE {
            assert!(
                matches!(*ext, "mp4" | "m4v" | "mov" | "webm"),
                "{ext} is offered but no demuxer here reads it"
            );
        }
    }

    #[test]
    fn directories_sort_before_files_and_each_group_is_alphabetical() {
        let v = sort_entries(vec![
            e("zeta.mp4", false),
            e("Beta", true),
            e("alpha.mp4", false),
            e("acme", true),
        ]);
        let names: Vec<&str> = v.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["acme", "Beta", "alpha.mp4", "zeta.mp4"]);
    }

    #[test]
    fn an_empty_query_keeps_everything_in_order() {
        let v = sort_entries(vec![e("b.mp4", false), e("a", true)]);
        assert_eq!(filter(&v, ""), v);
        assert_eq!(filter(&v, "  "), v);
    }

    #[test]
    fn filtering_ranks_the_best_match_first() {
        let v = vec![
            e("holiday-2019.mp4", false),
            e("cat.mp4", false),
            e("concert.webm", false),
        ];
        let r = filter(&v, "cat");
        assert_eq!(r.first().map(|e| e.name.as_str()), Some("cat.mp4"), "{r:?}");
    }

    #[test]
    fn filtering_drops_non_matches() {
        let v = vec![e("cat.mp4", false), e("dog.mp4", false)];
        let r = filter(&v, "cat");
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn a_query_matching_nothing_gives_an_empty_list() {
        let v = vec![e("cat.mp4", false)];
        assert!(filter(&v, "zzzq").is_empty());
    }

    /// Reopening should land where the user was, not at the filesystem root.
    #[test]
    fn the_picker_starts_beside_the_open_file() {
        let dir = std::env::temp_dir();
        let f = dir.join("vidkit-start-dir-probe.mp4");
        assert_eq!(start_dir(Some(&f)), dir);
    }

    #[test]
    fn with_no_open_file_it_starts_somewhere_that_exists() {
        let d = start_dir(None);
        assert!(d.is_dir(), "start dir {d:?} does not exist");
    }

    /// A path whose parent is gone must not produce a directory that cannot be
    /// listed; it falls back rather than returning something broken.
    #[test]
    fn a_missing_parent_falls_back_instead_of_returning_it() {
        let f = std::path::Path::new("/nonexistent-xyzzy/deeper/clip.mp4");
        let d = start_dir(Some(f));
        assert!(d.is_dir(), "fell back to {d:?}, which does not exist");
    }

    #[test]
    fn reading_a_directory_lists_dirs_and_playable_files_only() {
        let base = std::env::temp_dir().join("vidkit-picker-test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::write(base.join("clip.mp4"), b"x").unwrap();
        std::fs::write(base.join("notes.txt"), b"x").unwrap();
        std::fs::write(base.join(".hidden.mp4"), b"x").unwrap();

        let v = read_dir(&base, false).unwrap();
        let names: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["sub", "clip.mp4"], "got {names:?}");

        let with_hidden = read_dir(&base, true).unwrap();
        assert!(
            with_hidden.iter().any(|e| e.name == ".hidden.mp4"),
            "hidden toggle did nothing"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_unreadable_directory_reports_rather_than_panicking() {
        let r = read_dir(std::path::Path::new("/nonexistent-xyzzy"), false);
        assert!(r.is_err());
    }
}
