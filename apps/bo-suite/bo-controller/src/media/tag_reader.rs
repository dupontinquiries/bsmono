// src/media/tag_reader.rs
//
// Reads embedded ID3 / Vorbis comment / MP4 atom / Opus tags from audio
// files using the pure-Rust `lofty` crate. The result is shaped as the same
// ParsedTrack the filename parser returns, so the rest of the pipeline
// doesn't care where the metadata came from.
//
// Strategy: prefer embedded tags when present (they're authoritative).
// Fall back to filename parsing when a file has no tags or has tags so
// sparse they tell us nothing. If tags are present but missing the title,
// we fill the title from the filename — a common case where the artist
// embedded a `TPE1` but left `TIT2` blank.

use std::path::Path;

use lofty::file::TaggedFileExt;
use lofty::tag::Accessor;

use super::parser::ParsedTrack;

/// Attempt to read embedded tags from a file. Returns None if the file has
/// no tags, can't be opened, or the tags contain no useful metadata.
pub fn read_tags(path: &Path) -> Option<ParsedTrack> {
    let tagged = lofty::read_from_path(path).ok()?;
    // primary_tag is the format-preferred slot (ID3v2 on MP3, Vorbis comment
    // on FLAC/OGG, etc). first_tag falls back to whatever's there.
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;

    let title  = tag.title().map(|s| s.to_string()).unwrap_or_default();
    let artist = tag.artist().map(|s| s.to_string()).unwrap_or_default();
    let album  = tag.album().map(|s| s.to_string()).filter(|s| !s.is_empty());
    let year   = tag.year().and_then(|y| u16::try_from(y).ok());
    let genre  = tag.genre().map(|s| s.to_string()).filter(|s| !s.is_empty());

    // No useful metadata at all → treat as no tags so filename parsing wins.
    if title.is_empty() && artist.is_empty() && album.is_none() && year.is_none() {
        return None;
    }

    // Split artist on common multi-artist separators that DAWs/taggers use.
    // ID3v2 strictly uses `/` but practice varies; covering the common ones
    // gives nicer artist-browse results without much risk of false splits.
    let artists: Vec<String> = if artist.is_empty() {
        Vec::new()
    } else {
        artist.split(|c: char| c == ';' || c == '/')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };

    let mut tags = Vec::new();
    if let Some(g) = genre {
        // Genres can be multi-valued too in ID3v2.4 ("Rock;Pop").
        for piece in g.split(|c: char| c == ';' || c == '/') {
            let t = piece.trim();
            if !t.is_empty() { tags.push(t.to_string()); }
        }
    }

    Some(ParsedTrack {
        artists,
        features: Vec::new(),  // featured-artist detection from tags is rare
        title,
        album,
        tags,
        year,
        confidence: 95,        // embedded tags are very reliable
    })
}

/// Read tags if present and useful; otherwise fall back to parsing the
/// filename. When tags are present but the title is empty (a surprisingly
/// common case where the encoder filled TPE1 but not TIT2), use the
/// filename-parser's title as a fill-in.
pub fn read_or_parse(path: &Path, filename: &str, extra_known: &[String]) -> ParsedTrack {
    match read_tags(path) {
        Some(t) if !t.title.is_empty() => t,
        Some(t) => {
            // Tags present but no title — borrow the title from the filename.
            let from_name = super::parser::parse(filename, extra_known);
            ParsedTrack {
                title: from_name.title,
                // Artists from tags are still better than filename — keep them.
                artists: if t.artists.is_empty() { from_name.artists } else { t.artists },
                ..t
            }
        }
        None => super::parser::parse(filename, extra_known),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // We can't ship a real audio file in tests; structural checks only.
    #[test]
    fn read_tags_returns_none_for_nonexistent() {
        let p = Path::new("/definitely/not/a/file.mp3");
        assert!(read_tags(p).is_none());
    }

    #[test]
    fn read_or_parse_falls_back_when_no_tags() {
        let p = Path::new("/definitely/not/a/file.mp3");
        let r = read_or_parse(p, "Artist - Title.mp3", &[]);
        // filename parser should give us a title at least.
        assert!(!r.title.is_empty());
    }

    /// Real-file test: only runs if a tagged FLAC is at /tmp/tagged.flac
    /// (produced once via ffmpeg with title/artist/album/date/genre tags).
    /// Skipping silently when absent keeps CI happy without committing
    /// a binary test fixture into the repo.
    #[test]
    fn reads_real_flac_tags_when_fixture_present() {
        let p = Path::new("/tmp/tagged.flac");
        if !p.exists() { return; }
        let t = read_tags(p).expect("tagged.flac should yield tags");
        assert_eq!(t.title, "Hyperballad");
        assert_eq!(t.album.as_deref(), Some("Post"));
        assert_eq!(t.year, Some(1995));
        // Multi-artist split on ';' should produce two entries.
        assert_eq!(t.artists.len(), 2, "expected 2 artists, got {:?}", t.artists);
        assert_eq!(t.artists[0], "Björk");
        assert_eq!(t.artists[1], "Antony Hegarty");
        // Multi-genre split: "Electronic;Trip Hop" → two tags.
        assert_eq!(t.tags.len(), 2, "expected 2 tags, got {:?}", t.tags);
        // Confidence should be high for embedded tags.
        assert!(t.confidence >= 90);
    }
}
