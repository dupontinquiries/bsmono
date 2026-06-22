// src/parser.rs
//
// Filename parser for the user's convention:
//
//   {artist1, artist2, ...} - {song} ({tag1, tag2, ...}) - {album}
//
// Real-world variants this needs to handle:
//
//   "Daft Punk - One More Time"
//   "Daft Punk - One More Time.mp3"
//   "Daft Punk, Justice - One More Time (Remix)"
//   "Aphex Twin - Windowlicker (Acapella, Instrumental)"
//   "Burial - Untrue (2007 - Hyperdub)"                  (album with year)
//   "Burial - Untrue (Hyperdub) - Untrue"                (album dash split)
//   "Earth, Wind & Fire - September"                     (artist with comma!)
//   "Various Artists - Track 5 (DJ Mix) [Pete Tong]"     (brackets too)
//   "01 - Daft Punk - One More Time"                     (leading track no.)
//   "DJ Shadow ft. Run The Jewels - Nobody Speak"        (feat. notation)
//
// Approach:
//   1. Strip extension, strip leading track number.
//   2. Pull out parenthesized AND bracketed groups as candidate tags.
//   3. Split remainder on " - " (with spaces; avoids splitting hyphenated names).
//   4. First segment -> artists. Apply known multi-word-artist guard list,
//      then split on ", " / " & " / " feat. " / " ft. " / " vs. " / " x ".
//   5. Last segment if 3+ segments -> album.
//   6. Middle (or 2nd if 2 segments) -> song.
//   7. Parsed tags: split each group on "," and "/" to get individual tags;
//      detect years as a separate metadata field.

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParsedTrack {
    pub artists: Vec<String>,
    pub features: Vec<String>,  // ft./feat./vs./x — extracted separately
    pub title: String,
    pub album: Option<String>,
    pub tags: Vec<String>,
    pub year: Option<u16>,
    /// Confidence 0..=100. Drops if we had to guess heavily.
    pub confidence: u8,
}

// ─── Known multi-word artists that contain commas ───────────────────────────
//
// This is intentionally a small list. Easy to extend by editing a json file
// at runtime — see load_extra_known_artists.
//
// Heuristic: if any of these appears as a prefix of the artist segment, we
// remove it before doing comma-splitting, then add it back as the first
// artist.

const KNOWN_COMMA_ARTISTS: &[&str] = &[
    // Contain commas — must be protected from comma-split
    "Earth, Wind & Fire",
    "Crosby, Stills & Nash",
    "Crosby, Stills, Nash & Young",
    "Emerson, Lake & Palmer",
    "Peter, Paul and Mary",
    "Tyler, The Creator",
    // Contain " and " — must be protected from " and "-split
    "Tegan and Sara",
    "Above & Beyond",   // protected from " & "-split
    "Hall & Oates",
    "Simon & Garfunkel",
    "Mumford & Sons",
];

// Allow runtime additions via a JSON file shipped alongside the binary.
/// Load extra known-artist strings from a user-editable JSON file. Reserved
/// for a future Settings page; not yet wired into the scanner.
#[allow(dead_code)]
pub fn load_extra_known_artists(path: &std::path::Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    serde_json::from_slice::<Vec<String>>(&bytes).unwrap_or_default()
}

// ─── Pre-compiled regexes ───────────────────────────────────────────────────

static LEADING_TRACK_NO: Lazy<Regex> = Lazy::new(|| {
    // "01 - ", "01. ", "01_", "001 - " at the start
    Regex::new(r"^\s*\d{1,3}\s*[-._]\s*").unwrap()
});

static PARENS_GROUP: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\(([^()]*)\)|\[([^\[\]]*)\]").unwrap()
});

static YEAR_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b(19[3-9]\d|20[0-4]\d)\b").unwrap()
});

static FEATURE_SPLIT: Lazy<Regex> = Lazy::new(|| {
    // splits "Artist1 ft. Artist2 & Artist3" -> ["Artist1", "Artist2", "Artist3"]
    // when used after we've handled commas.
    Regex::new(r"(?i)\s+(?:ft\.?|feat\.?|featuring|vs\.?|with)\s+").unwrap()
});

// ─── Main entry point ───────────────────────────────────────────────────────

pub fn parse(filename: &str, extra_known: &[String]) -> ParsedTrack {
    let mut confidence = 100u8;

    // 1. Strip extension
    let stem = strip_extension(filename);

    // 2. Strip leading track number
    let stem = LEADING_TRACK_NO.replace(&stem, "").to_string();

    // 3. Extract parenthesized/bracketed groups as tags + year
    let (without_groups, tag_groups) = extract_groups(&stem);

    let mut tags = Vec::new();
    let mut year = None;
    for group in &tag_groups {
        for t in split_tag_group(group) {
            // Year detection
            if let Some(m) = YEAR_RE.find(&t) {
                if let Ok(y) = m.as_str().parse::<u16>() {
                    year = Some(y);
                    // remove year-only tags; if other text exists, keep it
                    let cleaned = YEAR_RE.replace_all(&t, "").trim().to_string();
                    if !cleaned.is_empty() {
                        tags.push(cleaned);
                    }
                    continue;
                }
            }
            tags.push(t);
        }
    }

    // 4. Dash-separated segments
    let segments: Vec<String> = without_groups
        .split(" - ")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let (artists_seg, title_seg, album_seg): (String, String, Option<String>) = match segments.len() {
        0 => {
            confidence = 10;
            (String::new(), without_groups.trim().to_string(), None)
        }
        1 => {
            // No " - " found — treat the whole thing as title
            confidence = 50;
            (String::new(), segments.into_iter().next().unwrap(), None)
        }
        2 => {
            let mut it = segments.into_iter();
            let a = it.next().unwrap();
            let t = it.next().unwrap();
            (a, t, None)
        }
        _ => {
            // 3+ segments: first is artists, last is album, middle joined as title
            let n = segments.len();
            let artist_seg = segments[0].clone();
            let title = segments[1..n - 1].join(" - ");
            let album = segments[n - 1].clone();
            (artist_seg, title, Some(album))
        }
    };

    // 5. Parse the artist segment
    let (artists, features) = parse_artists(&artists_seg, extra_known);
    if artists.is_empty() && !artists_seg.is_empty() {
        confidence = confidence.saturating_sub(20);
    }

    // 6. Light cleanup of title
    let title = clean_title(&title_seg);

    // 7. Album sanity check
    let album = album_seg.and_then(|a| {
        let a = a.trim().to_string();
        if a.is_empty() { None } else { Some(a) }
    });

    ParsedTrack {
        artists,
        features,
        title,
        album,
        tags,
        year,
        confidence,
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn strip_extension(s: &str) -> String {
    if let Some(dot) = s.rfind('.') {
        // only strip recognized audio extensions
        let ext = &s[dot + 1..].to_lowercase();
        if matches!(ext.as_str(),
            "mp3"|"flac"|"m4a"|"aac"|"ogg"|"opus"|"wav"|"wma"|"aiff"|"alac")
        {
            return s[..dot].to_string();
        }
    }
    s.to_string()
}

/// Returns (string with groups removed, list of group contents).
fn extract_groups(s: &str) -> (String, Vec<String>) {
    let mut groups = Vec::new();
    let cleaned = PARENS_GROUP.replace_all(s, |caps: &regex::Captures| {
        let inner = caps.get(1).or_else(|| caps.get(2)).map(|m| m.as_str()).unwrap_or("");
        if !inner.trim().is_empty() {
            groups.push(inner.trim().to_string());
        }
        " ".to_string()  // collapse to a space so split logic stays clean
    });
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    (collapsed, groups)
}

fn split_tag_group(group: &str) -> Vec<String> {
    group
        .split(|c| c == ',' || c == '/')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() < 60)  // wildly long entries aren't tags
        .collect()
}

fn parse_artists(seg: &str, extra: &[String]) -> (Vec<String>, Vec<String>) {
    if seg.is_empty() { return (vec![], vec![]); }

    // Pull out features (ft./feat./vs./with) — they ride separately
    let mut features: Vec<String> = Vec::new();
    let parts: Vec<&str> = FEATURE_SPLIT.split(seg).collect();
    let main_segment = parts[0];
    for p in &parts[1..] {
        for sub in split_artist_segment(p, extra) {
            features.push(sub);
        }
    }

    let main = split_artist_segment(main_segment, extra);
    (main, features)
}

fn split_artist_segment(seg: &str, extra: &[String]) -> Vec<String> {
    let seg = seg.trim();
    if seg.is_empty() { return vec![]; }

    // Check known multi-word artists first
    let mut known: Vec<String> = KNOWN_COMMA_ARTISTS.iter().map(|s| s.to_string()).collect();
    known.extend_from_slice(extra);
    known.sort_by_key(|s| std::cmp::Reverse(s.len()));   // longest match wins

    for k in &known {
        if let Some(stripped) = strip_prefix_ci(seg, k) {
            let trimmed = stripped.trim_start_matches(|c: char| c == ',' || c == ' ' || c == '&').trim();
            let mut out = vec![k.clone()];
            if !trimmed.is_empty() {
                out.extend(split_simple(trimmed));
            }
            return out;
        }
    }

    split_simple(seg)
}

fn split_simple(seg: &str) -> Vec<String> {
    // Split on commas first, then on " & " inside each piece, then on " and "
    let mut out = Vec::new();
    for piece in seg.split(',') {
        for sub in piece.split(" & ") {
            for sub2 in sub.split(" and ") {
                let s = sub2.trim();
                if !s.is_empty() {
                    out.push(s.to_string());
                }
            }
        }
    }
    out
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    // UTF-8 safety: if the byte index lands inside a multi-byte character
    // (e.g. "Juice WRLD – Deja Vu" where '–' is 3 bytes), slicing panics.
    // Reject quickly when prefix.len() isn't a char boundary, and only
    // compare against the ASCII portion of s.
    if s.len() < prefix.len() { return None; }
    if !s.is_char_boundary(prefix.len()) { return None; }
    if s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn clean_title(s: &str) -> String {
    s.trim().to_string()
}

// ─── Normalization for indexing/grouping ────────────────────────────────────

/// Lowercase + strip non-alphanumeric. Used to build a stable grouping key
/// so "The Beatles" and "the beatles" group together, and to dedupe
/// near-identical artist names.
pub fn normalize_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> ParsedTrack { parse(s, &[]) }

    #[test]
    fn simple() {
        let r = p("Daft Punk - One More Time.mp3");
        assert_eq!(r.artists, vec!["Daft Punk"]);
        assert_eq!(r.title, "One More Time");
        assert!(r.album.is_none());
        assert!(r.tags.is_empty());
    }

    #[test]
    fn multi_artist() {
        let r = p("Daft Punk, Justice - One More Time");
        assert_eq!(r.artists, vec!["Daft Punk", "Justice"]);
    }

    #[test]
    fn known_comma_artist() {
        let r = p("Earth, Wind & Fire - September");
        assert_eq!(r.artists, vec!["Earth, Wind & Fire"]);
        assert_eq!(r.title, "September");
    }

    #[test]
    fn known_comma_artist_with_feature() {
        let r = p("Earth, Wind & Fire, Daft Punk - September");
        assert_eq!(r.artists, vec!["Earth, Wind & Fire", "Daft Punk"]);
    }

    #[test]
    fn tags_and_album() {
        let r = p("Aphex Twin - Windowlicker (Acapella, Instrumental) - Selected Ambient");
        assert_eq!(r.artists, vec!["Aphex Twin"]);
        assert_eq!(r.title, "Windowlicker");
        assert_eq!(r.tags, vec!["Acapella", "Instrumental"]);
        assert_eq!(r.album.as_deref(), Some("Selected Ambient"));
    }

    #[test]
    fn year_extraction() {
        let r = p("Burial - Untrue (2007 - Hyperdub)");
        assert_eq!(r.year, Some(2007));
    }

    #[test]
    fn feat_notation() {
        let r = p("DJ Shadow ft. Run The Jewels - Nobody Speak");
        assert_eq!(r.artists, vec!["DJ Shadow"]);
        assert_eq!(r.features, vec!["Run The Jewels"]);
    }

    #[test]
    fn leading_track_no() {
        let r = p("01 - Daft Punk - One More Time");
        assert_eq!(r.artists, vec!["Daft Punk"]);
        assert_eq!(r.title, "One More Time");
    }

    #[test]
    fn brackets_as_tags() {
        let r = p("Various Artists - Track 5 (DJ Mix) [Pete Tong]");
        assert!(r.tags.contains(&"DJ Mix".to_string()));
        assert!(r.tags.contains(&"Pete Tong".to_string()));
    }
}
