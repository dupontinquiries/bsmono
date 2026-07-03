// src/media/tag_writer.rs
//
// Write user metadata edits back into the actual audio file's tags, so an
// edit made in bo-controller is portable: it survives a library wipe and
// shows up in every other player that reads the file. This is the write
// counterpart to tag_reader.rs.
//
// Design principles, because this MODIFIES the user's files:
//
//   * Least surprise. We only touch the fields the user actually changed
//     (title / artist / album / year). Every other tag, embedded art,
//     and unrelated frame is preserved — we read the existing tag, mutate
//     a few fields, and write it back, rather than constructing a fresh
//     tag from scratch.
//
//   * An empty edit field CLEARS that field (set to ""), matching what the
//     edit dialog shows; we don't silently keep a stale value. (The caller
//     decides which fields to include — see TagEdit's Options.)
//
//   * Best-effort + total honesty. Any failure (read-only file, unsupported
//     container, no writable tag) returns an Err the caller surfaces; we
//     never panic and never half-write. lofty writes atomically per its
//     own implementation.
//
//   * Rating is intentionally NOT written here. A 0–5 rating has no single
//     portable home across ID3 (POPM, 0–255 scaled), Vorbis comments
//     (RATING, free-text), and MP4 (custom atom) — each player interprets
//     them differently, so writing one risks corrupting another tool's
//     view. Ratings stay in our DB (already durable via the path mirror).
//     A format-aware rating writer can come later if wanted.

use std::path::Path;

use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::tag::{Accessor, TagExt};

/// Which fields to write. A `Some` value is written (empty string clears
/// the field); `None` leaves the file's existing value untouched. This
/// lets the caller distinguish "user cleared this" from "user didn't
/// touch this".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TagEdit {
    pub title:  Option<String>,
    pub artist: Option<String>,
    pub album:  Option<String>,
    pub year:   Option<u32>,
}

impl TagEdit {
    /// True if nothing would be written — lets the caller skip touching
    /// the file at all.
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.artist.is_none()
            && self.album.is_none() && self.year.is_none()
    }
}

/// Apply a TagEdit to a file's tags in place, preserving everything else.
/// Returns Ok(()) on a successful write, or an Err describing why not.
pub fn write_tags(path: &Path, edit: &TagEdit) -> Result<(), String> {
    if edit.is_empty() {
        return Ok(()); // nothing to do — not an error.
    }
    if !path.exists() {
        return Err(format!("file not found: {}", path.display()));
    }

    let mut tagged = lofty::read_from_path(path)
        .map_err(|e| format!("read tags: {}", e))?;

    // Work on the format-preferred tag if present; otherwise create one in
    // the file's primary tag type so the write has somewhere to land.
    if tagged.primary_tag_mut().is_none() {
        let tag_type = tagged.primary_tag_type();
        tagged.insert_tag(lofty::tag::Tag::new(tag_type));
    }
    let tag = tagged.primary_tag_mut()
        .ok_or_else(|| "no writable tag slot for this format".to_string())?;

    if let Some(t) = &edit.title {
        if t.is_empty() { tag.remove_title(); } else { tag.set_title(t.clone()); }
    }
    if let Some(a) = &edit.artist {
        if a.is_empty() { tag.remove_artist(); } else { tag.set_artist(a.clone()); }
    }
    if let Some(al) = &edit.album {
        if al.is_empty() { tag.remove_album(); } else { tag.set_album(al.clone()); }
    }
    if let Some(y) = edit.year {
        if y == 0 { tag.remove_year(); } else { tag.set_year(y); }
    }

    tag.save_to_path(path, WriteOptions::default())
        .map_err(|e| format!("write tags to {}: {}", path.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_edit_is_noop_ok() {
        // An empty edit writes nothing and succeeds without touching disk
        // (the path doesn't even need to exist).
        let edit = TagEdit::default();
        assert!(edit.is_empty());
        assert!(write_tags(Path::new("/nonexistent/never.mp3"), &edit).is_ok());
    }

    #[test]
    fn missing_file_with_real_edit_errors() {
        let edit = TagEdit { title: Some("X".into()), ..Default::default() };
        assert!(!edit.is_empty());
        let r = write_tags(Path::new("/nonexistent/never.mp3"), &edit);
        assert!(r.is_err(), "writing to a missing file should error");
    }

    #[test]
    fn is_empty_reflects_fields() {
        assert!(TagEdit::default().is_empty());
        assert!(!TagEdit { year: Some(2020), ..Default::default() }.is_empty());
        assert!(!TagEdit { title: Some(String::new()), ..Default::default() }.is_empty(),
            "an explicit clear (empty string) is still a write");
    }

    // A real round-trip (write then read back) requires an on-disk audio
    // fixture; gated on its presence like tag_reader's FLAC test so the
    // suite stays hermetic.
    #[test]
    fn round_trip_when_fixture_present() {
        let fixture = std::path::PathBuf::from(
            env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/writable.flac");
        if !fixture.exists() { return; } // skip when no fixture
        // Copy to a temp so we don't mutate the fixture.
        let tmp = std::env::temp_dir().join("bo_tagwrite_test.flac");
        let _ = std::fs::copy(&fixture, &tmp);
        let edit = TagEdit {
            title: Some("Edited Title".into()),
            artist: Some("Edited Artist".into()),
            ..Default::default()
        };
        write_tags(&tmp, &edit).unwrap();
        let back = super::super::tag_reader::read_tags(&tmp).unwrap();
        assert_eq!(back.title, "Edited Title");
        assert!(back.artists.iter().any(|a| a == "Edited Artist"));
        let _ = std::fs::remove_file(&tmp);
    }
}
