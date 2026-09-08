//! note_store — clipped notes on disk, as the markdown they already render to.
//!
//! `TODO_notes_clip.md`'s "Persist + view" section: *"Notes saved as .md under a notes dir;
//! the web view already renders markdown → 'add to the web' = open the note's .md"*. The
//! data model and `Note::to_markdown` have been pure and tested since v0.1.641; nothing ever
//! wrote the result anywhere, so a session's clips died with the process.
//!
//! ## Markdown, not a private format
//!
//! The doc's reason is the good one: the web view already renders markdown, so a saved note
//! is readable by the thing that would show it, by any editor, and by the operator's own
//! `grep`. A note format nobody else can read is a note you can only get at through the app
//! that made it — which is the opposite of what a clipping tool is for.
//!
//! It costs something: a note read back is markdown, not the `Note` tree, so the block
//! structure and a clip's provenance are readable but not reconstructible. That is the
//! deliberate trade, and [`load_markdown`] returns a `String` rather than pretending to
//! parse one back.
//!
//! ## Filenames are derived, and collisions are not overwritten
//!
//! A title becomes a slug. Two notes called "build notes" on the same day would otherwise
//! be one file, and the second silently replacing the first is the worst outcome available —
//! so a suffix is added instead.

/// Where notes live: `$XDG_DATA_HOME/voiceterm-egui/notes/` (or `~/.local/share/...`).
///
/// DATA, not config. A clipped note is something the operator made, and config directories
/// get hand-edited, synced and reset in ways nobody expects their notes to follow.
pub fn notes_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("voiceterm-egui").join("notes")
}

/// A filesystem-safe stem for a title.
///
/// Lower-cased, non-alphanumerics collapsed to a single `-`, trimmed. An empty or
/// all-punctuation title becomes `note` rather than an empty name — a file called `.md` is
/// hidden on every unix and looks like the save failed.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in title.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "note".to_string()
    } else {
        // Long titles make unwieldy names and some filesystems cap a component at 255
        // bytes; a truncated slug is still legible and still unique via the suffix below.
        trimmed.chars().take(60).collect::<String>().trim_matches('-').to_string()
    }
}

/// The path a note with this title would take in `dir`, avoiding an existing file.
///
/// `note.md`, then `note-2.md`, `note-3.md`… Two notes called "build notes" would otherwise
/// be one file, and the second silently replacing the first is the worst outcome available.
pub fn free_path(dir: &std::path::Path, title: &str) -> std::path::PathBuf {
    let stem = slug(title);
    let first = dir.join(format!("{stem}.md"));
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let p = dir.join(format!("{stem}-{n}.md"));
        if !p.exists() {
            return p;
        }
    }
    // A thousand notes with one title. Take the last rather than loop forever; whoever gets
    // here has a bigger problem than a clobbered file.
    dir.join(format!("{stem}-1000.md"))
}

/// Rewrite a note at a path it already owns.
///
/// A note GROWS: every clip appends a block and the whole thing is re-rendered. Calling
/// [`save`] each time would ask `free_path` for a fresh name and scatter one note across
/// `build-notes.md`, `build-notes-2.md`, `build-notes-3.md` — one file per clip, each a
/// prefix of the next. The caller keeps the path from the first save and passes it back.
pub fn save_to(path: &std::path::Path, note: &crate::note_clip::Note) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, note.to_markdown())
}

/// Write a note's markdown into [`notes_dir`], returning where it went.
pub fn save(note: &crate::note_clip::Note) -> std::io::Result<std::path::PathBuf> {
    save_in(&notes_dir(), note)
}

/// [`save`] into a given directory — the same function the tests drive, so what is tested is
/// what runs.
pub fn save_in(
    dir: &std::path::Path,
    note: &crate::note_clip::Note,
) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = free_path(dir, &note.title);
    std::fs::write(&path, note.to_markdown())?;
    Ok(path)
}

/// Every saved note, newest first by modification time.
///
/// Name and path only: a picker needs to list a hundred notes and reading them all to show
/// their titles would be a hundred file reads for a menu.
pub fn list_in(dir: &std::path::Path) -> Vec<(String, std::path::PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<(std::time::SystemTime, String, std::path::PathBuf)> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let when = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        out.push((when, name, p));
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().map(|(_, n, p)| (n, p)).collect()
}

/// The notes in [`notes_dir`].
pub fn list() -> Vec<(String, std::path::PathBuf)> {
    list_in(&notes_dir())
}

/// Every saved note as `(name, title, markdown)`, newest first.
///
/// For a reader that wants documents rather than a list — the notes join the shipped docs in
/// the same view, so "add to the web" is opening a note in the thing that already renders
/// markdown rather than a second viewer that renders it slightly differently.
///
/// A note that cannot be read is SKIPPED, not surfaced as an entry that fails when opened: a
/// file removed between the listing and the read is ordinary, and a reader offering a
/// document it cannot show is worse than one that does not offer it.
///
/// The name is prefixed so a note called "readme" cannot displace the shipped README in a
/// view keyed by name.
pub fn documents_in(dir: &std::path::Path) -> Vec<(String, String, String)> {
    list_in(dir)
        .into_iter()
        .filter_map(|(name, path)| {
            let md = load_markdown(&path).ok()?;
            Some((format!("note:{name}"), title_of(&md).unwrap_or(name), md))
        })
        .collect()
}

/// The notes in [`notes_dir`], as documents.
pub fn documents() -> Vec<(String, String, String)> {
    documents_in(&notes_dir())
}

/// A note's own title, from its first markdown heading.
///
/// The file name is a slug — lower-cased and hyphenated — and showing "build-notes" in a
/// reader when the note says "Build notes" is a worse label than the one the note carries.
pub fn title_of(md: &str) -> Option<String> {
    md.lines()
        .find_map(|l| l.strip_prefix("# ").map(str::trim))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// A saved note's markdown, as text.
///
/// Text, not a `Note`: see the module docs. Reconstructing the tree would need a markdown
/// parser that agreed exactly with `to_markdown`, and a round trip that is nearly right is
/// worse than one that is honestly one-way.
pub fn load_markdown(path: &std::path::Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note_clip::Note;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("vt-notes-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    /// The whole point: a note survives the process. Nothing wrote `to_markdown`'s output
    /// anywhere, so a session's clips died with it.
    #[test]
    fn a_note_is_written_as_readable_markdown() {
        let dir = scratch("save");
        let mut n = Note::new("Build notes");
        n.push_prose("the linker was the problem");
        let path = save_in(&dir, &n).expect("saved");

        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("md"));
        assert_eq!(path.file_name().unwrap(), "build-notes.md", "the title should name the file");
        let text = load_markdown(&path).expect("read back");
        assert!(text.contains("the linker was the problem"), "{text}");
        assert_eq!(text, n.to_markdown(), "the file is the note's own markdown, unaltered");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A note GROWS. Saving each clip through `save` would ask for a fresh name every time
    /// and scatter one note across `build-notes.md`, `-2`, `-3` — one file per clip, each a
    /// prefix of the next.
    #[test]
    fn a_growing_note_rewrites_its_own_file() {
        let dir = scratch("grow");
        let mut n = Note::new("build notes");
        n.push_prose("first thought");
        let path = save_in(&dir, &n).expect("saved");

        n.push_prose("second thought");
        save_to(&path, &n).expect("rewritten");

        let text = load_markdown(&path).unwrap();
        assert!(text.contains("first thought") && text.contains("second thought"), "{text}");
        assert_eq!(list_in(&dir).len(), 1, "the note was scattered across files");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two notes with one title is ordinary. The second silently replacing the first is the
    /// worst outcome available.
    #[test]
    fn a_second_note_with_the_same_title_does_not_replace_the_first() {
        let dir = scratch("collide");
        let mut a = Note::new("build notes");
        a.push_prose("first");
        let mut b = Note::new("build notes");
        b.push_prose("second");

        let pa = save_in(&dir, &a).unwrap();
        let pb = save_in(&dir, &b).unwrap();
        assert_ne!(pa, pb, "the second note overwrote the first");
        assert!(load_markdown(&pa).unwrap().contains("first"), "the first note was clobbered");
        assert!(load_markdown(&pb).unwrap().contains("second"));
        assert_eq!(pb.file_name().unwrap(), "build-notes-2.md");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file called `.md` is hidden on every unix and looks like the save failed.
    #[test]
    fn an_unusable_title_still_produces_a_usable_filename() {
        assert_eq!(slug(""), "note");
        assert_eq!(slug("???"), "note");
        assert_eq!(slug("   "), "note");
        // Spaces and punctuation collapse rather than repeat.
        assert_eq!(slug("Build   notes: part 2!"), "build-notes-part-2");
        assert_eq!(slug("--leading and trailing--"), "leading-and-trailing");
        // Long titles are cut, and not left ending in a dash.
        let long = slug(&"word ".repeat(40));
        assert!(long.len() <= 60 && !long.ends_with('-'), "{long}");

        let dir = scratch("slug");
        let p = save_in(&dir, &Note::new("???")).unwrap();
        assert_eq!(p.file_name().unwrap(), "note.md");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A picker wants the newest first, and wants it without reading a hundred files.
    #[test]
    fn listing_is_newest_first_and_ignores_what_is_not_a_note() {
        let dir = scratch("list");
        std::fs::create_dir_all(&dir).unwrap();
        for t in ["one", "two", "three"] {
            let mut n = Note::new(t);
            n.push_prose(t);
            save_in(&dir, &n).unwrap();
            // mtime resolution is coarse enough that same-millisecond writes tie.
            std::thread::sleep(std::time::Duration::from_millis(12));
        }
        // Not a note, and must not appear in a note picker.
        std::fs::write(dir.join("clips.png"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();

        let names: Vec<String> = list_in(&dir).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["three".to_string(), "two".to_string(), "one".to_string()], "{names:?}");

        // A directory that does not exist is an empty list, not an error: a first run has no
        // notes and that is not a failure.
        assert!(list_in(&dir.join("nope")).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The reader shows documents, not paths. A note that cannot be read is skipped rather
    /// than offered as an entry that fails when opened.
    #[test]
    fn notes_become_documents_with_their_own_titles() {
        let dir = scratch("docs");
        let mut n = Note::new("Build notes");
        n.push_prose("the linker again");
        save_in(&dir, &n).unwrap();

        let docs = documents_in(&dir);
        assert_eq!(docs.len(), 1);
        let (name, title, md) = &docs[0];
        // Prefixed, so a note called "readme" cannot displace the shipped README in a view
        // keyed by name.
        assert!(name.starts_with("note:"), "{name}");
        // The note's OWN title, not the slug: showing "build-notes" when the note says
        // "Build notes" is a worse label than the one it carries.
        assert_eq!(title, "Build notes");
        assert!(md.contains("the linker again"));

        // Something that is not a note is not a document.
        std::fs::write(dir.join("stray.png"), b"x").unwrap();
        assert_eq!(documents_in(&dir).len(), 1);

        // A title-less note falls back to its file name rather than showing nothing.
        std::fs::write(dir.join("untitled.md"), b"just some text\n").unwrap();
        let docs = documents_in(&dir);
        assert!(docs.iter().any(|(_, t, _)| t == "untitled"), "{docs:?}");

        assert_eq!(title_of("# Hello\n\nbody"), Some("Hello".to_string()));
        assert_eq!(title_of("no heading"), None);
        assert_eq!(title_of("#  \n"), None, "an empty heading is not a title");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Notes are DATA. Config directories get hand-edited, synced and reset in ways nobody
    /// expects their notes to follow.
    #[test]
    fn notes_live_under_the_data_dir() {
        let p = notes_dir();
        let s = p.to_string_lossy();
        assert!(s.ends_with("voiceterm-egui/notes"), "{s}");
        assert!(!s.contains("/.config/"), "notes should not be config: {s}");
    }
}
