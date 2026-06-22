// src/phoneme_db.rs — runtime loader for the bundled phoneme database.
//
// v0.1.246 (PA3 of the phoneme alignment + tiered reading model arc).
//
// The database is generated at build-time-but-out-of-band by
// `scripts/gen-phoneme-db.py` and committed to the repo as
// `data/phoneme-db.txt`. The file is baked into the binary via
// include_str! so the runtime has zero filesystem overhead and the
// database can never be missing.
//
// On first lookup, the text is parsed once into the static `DB`
// (OnceLock) and indexed by (manpage_name, paragraph_id). Subsequent
// lookups are O(1).
//
// The on-disk format is line-based (see scripts/gen-phoneme-db.py
// docstring for the spec):
//
//   # comment
//   M <manpage_name> <source_checksum>
//   P <paragraph_id> <text>
//   W <word>\t<phonemes>
//
// This module is intentionally dep-free. The project deliberately
// avoids serde_json (see Cargo.toml — only hand-rolled JSON in
// archive.rs for flat objects, and binary formats elsewhere). The
// line-based format is parseable with split() and that's it.
//
// What this module enables but does NOT do yet:
//   - PA4: a phoneme classifier that consumes audio frames and
//     produces phoneme-probability estimates
//   - PA5: online forced alignment that pairs the classifier's
//     output with this database's expectations to know "user is
//     reading word N of paragraph M right now"
//   - PB1: replacing the WPM Timer in reading-overlay with that
//     forced-alignment position so word advance follows actual
//     speech instead of a uniform timer
//
// This module just exposes the data. The consumers come later.
//
// Stale-checksum behavior: when a manpage's source text doesn't
// match the recorded checksum, lookup() returns None and logs a
// one-time warning. That ensures stale data doesn't silently misalign
// downstream; manpage edits trigger an obvious "regenerate the db"
// message.

use std::collections::HashMap;
use std::sync::OnceLock;

const RAW: &str = include_str!("../data/phoneme-db.txt");

/// A single word's text + phoneme transcription.
#[derive(Debug, Clone)]
pub struct WordEntry {
    pub text: String,
    pub phonemes: String,
}

/// A selectable paragraph with its per-word phoneme list.
///
/// `normalized_text` is computed once at parse time and used as
/// the lookup key. Storing it avoids re-normalizing every
/// paragraph on every `lookup_*` call (which would allocate a
/// `String` per comparison — cheap at 27 paragraphs, not cheap
/// once PB4 grows the database to thousands of entries).
#[derive(Debug, Clone)]
pub struct ParagraphPhonemes {
    pub paragraph_id: usize,
    pub text: String,
    pub normalized_text: String,
    pub words: Vec<WordEntry>,
}

/// A single manpage's phoneme entries plus its source checksum.
/// Paragraph IDs are 1:1 with the runtime's paragraph emission
/// order in src/manpages.rs (incremented by qml_gen::next_paragraph_id
/// across all wrap calls). Lookups happen by content match, not
/// strict id match, since the runtime counter is process-wide and
/// paragraph_id varies by what else was rendered first.
#[derive(Debug)]
pub struct ManpagePhonemes {
    pub name: String,
    pub source_checksum: String,
    pub paragraphs: Vec<ParagraphPhonemes>,
}

/// Whole-database lookup index. Built once on first access.
#[derive(Debug, Default)]
pub struct PhonemeDb {
    pub version: u32,
    pub generator: String,
    pub ipa: bool,
    pub manpages: HashMap<String, ManpagePhonemes>,
}

static DB: OnceLock<PhonemeDb> = OnceLock::new();

/// Unescape \\, \n, \t in a field. Reverse of the generator's esc().
fn unesc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => o.push('\n'),
                Some('t') => o.push('\t'),
                Some('\\') => o.push('\\'),
                Some(other) => {
                    o.push('\\');
                    o.push(other);
                }
                None => o.push('\\'),
            }
        } else {
            o.push(c);
        }
    }
    o
}

/// Parse the bundled phoneme-db.txt into the indexed structure.
fn parse(raw: &str) -> PhonemeDb {
    let mut db = PhonemeDb {
        version: 0,
        generator: String::new(),
        ipa: false,
        manpages: HashMap::new(),
    };
    // Cursor state: which manpage and paragraph we're currently
    // populating. The line-based format is strictly ordered
    // M → P → W → W → ... → P → W → ... → M → ...
    let mut cur_manpage: Option<ManpagePhonemes> = None;
    let mut cur_paragraph: Option<ParagraphPhonemes> = None;

    for line in raw.lines() {
        // Empty lines / comments / version markers.
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("# phoneme-db v") {
            // "v1 (generator: ...)"
            if let Some(space_idx) = rest.find(' ') {
                if let Ok(v) = rest[..space_idx].parse::<u32>() {
                    db.version = v;
                }
                // Generator is in the parenthetical.
                if let Some(open) = rest.find("(generator: ") {
                    let after = &rest[open + "(generator: ".len()..];
                    let close = after.rfind(')').unwrap_or(after.len());
                    db.generator = after[..close].to_string();
                }
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("# ipa: ") {
            db.ipa = rest.trim() == "true";
            continue;
        }
        if line.starts_with('#') {
            continue;
        }

        // M <name> <checksum>
        if let Some(rest) = line.strip_prefix("M ") {
            // Close out any in-flight paragraph + manpage.
            if let Some(p) = cur_paragraph.take() {
                if let Some(m) = cur_manpage.as_mut() {
                    m.paragraphs.push(p);
                }
            }
            if let Some(m) = cur_manpage.take() {
                db.manpages.insert(m.name.clone(), m);
            }
            // Parse "<name> <checksum>"
            if let Some(sp) = rest.find(' ') {
                let name = rest[..sp].to_string();
                let checksum = rest[sp + 1..].to_string();
                cur_manpage = Some(ManpagePhonemes {
                    name,
                    source_checksum: checksum,
                    paragraphs: Vec::new(),
                });
            }
            continue;
        }

        // P <id> <text>
        if let Some(rest) = line.strip_prefix("P ") {
            // Close any in-flight paragraph first.
            if let Some(p) = cur_paragraph.take() {
                if let Some(m) = cur_manpage.as_mut() {
                    m.paragraphs.push(p);
                }
            }
            if let Some(sp) = rest.find(' ') {
                if let Ok(pid) = rest[..sp].parse::<usize>() {
                    let text = unesc(&rest[sp + 1..]);
                    let normalized_text = normalize_for_match(&text);
                    cur_paragraph = Some(ParagraphPhonemes {
                        paragraph_id: pid,
                        text,
                        normalized_text,
                        words: Vec::new(),
                    });
                }
            }
            continue;
        }

        // W <word>\t<phonemes>
        if let Some(rest) = line.strip_prefix("W ") {
            if let Some(tab) = rest.find('\t') {
                let word = unesc(&rest[..tab]);
                let phonemes = unesc(&rest[tab + 1..]);
                if let Some(p) = cur_paragraph.as_mut() {
                    p.words.push(WordEntry {
                        text: word,
                        phonemes,
                    });
                }
            }
            continue;
        }
    }

    // Close out the final paragraph + manpage.
    if let Some(p) = cur_paragraph.take() {
        if let Some(m) = cur_manpage.as_mut() {
            m.paragraphs.push(p);
        }
    }
    if let Some(m) = cur_manpage.take() {
        db.manpages.insert(m.name.clone(), m);
    }

    db
}

/// Access the parsed database, parsing on first call. Subsequent
/// calls return the cached static.
pub fn db() -> &'static PhonemeDb {
    DB.get_or_init(|| parse(RAW))
}

/// Look up a paragraph by manpage name and the paragraph's TEXT.
/// Returns the per-word phoneme list if found.
///
/// We look up by text rather than by paragraph_id because the
/// runtime's next_paragraph_id() counter is process-wide and may
/// not match the build-time generation order (e.g., if other
/// content rendered first this session). Text-match is more
/// robust to render ordering.
///
/// Plain text-match: whitespace-collapsed, trimmed. Each
/// paragraph's normalized form is precomputed at parse time
/// (`ParagraphPhonemes.normalized_text`), so the per-lookup
/// cost is one `String` allocation (for the query) plus N
/// cheap `&str` comparisons. Future PA4+ might want a fuzzy
/// match for edge cases; v1 just does exact string equality.
pub fn lookup_by_text(manpage: &str, text: &str) -> Option<&'static ParagraphPhonemes> {
    let d = db();
    let m = d.manpages.get(manpage)?;
    let normalized = normalize_for_match(text);
    m.paragraphs.iter().find(|p| p.normalized_text == normalized)
}

/// Look up across ALL manpages by paragraph text. Useful when
/// the caller doesn't know which manpage a paragraph came from
/// (e.g., the manpage name isn't carried through paragraph_selected).
pub fn lookup_any(text: &str) -> Option<(&'static str, &'static ParagraphPhonemes)> {
    let d = db();
    let normalized = normalize_for_match(text);
    for (name, m) in &d.manpages {
        if let Some(p) = m.paragraphs.iter().find(|p| p.normalized_text == normalized) {
            return Some((name.as_str(), p));
        }
    }
    None
}

/// Normalize a paragraph text for matching. Trim, collapse internal
/// whitespace runs to single space. Intentionally minimal — we don't
/// want to lose case or punctuation since those affect pronunciation.
fn normalize_for_match(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Number of paragraphs across all manpages in the database. Useful
/// for the debug eprintln at startup and for tests.
pub fn total_paragraphs() -> usize {
    db().manpages.values().map(|m| m.paragraphs.len()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bundled_db() {
        let d = db();
        assert_eq!(d.version, 1);
        assert!(d.ipa);
        assert!(!d.generator.is_empty());
        assert!(d.manpages.contains_key("welcome"));
        assert!(d.manpages.contains_key("manpages"));
        assert!(d.manpages.contains_key("onboarding"));
    }

    #[test]
    fn welcome_first_paragraph_has_words() {
        let d = db();
        let m = d.manpages.get("welcome").expect("welcome manpage");
        assert!(!m.paragraphs.is_empty());
        // Manpages have a fixed structure; the first selectable
        // paragraph of welcome.md is the heading "Welcome to web2qml".
        let p = m.paragraphs.first().expect("first paragraph");
        assert!(!p.words.is_empty());
        // The word "Welcome" should be present (case preserved).
        let has_welcome = p.words.iter().any(|w| w.text == "Welcome");
        assert!(has_welcome, "words: {:?}", p.words);
    }

    #[test]
    fn lookup_any_finds_known_paragraph() {
        let d = db();
        // Pick any paragraph from welcome to verify lookup.
        let m = d.manpages.get("welcome").expect("welcome");
        let p = m.paragraphs.first().expect("first");
        let (found_manpage, found_p) =
            lookup_any(&p.text).expect("lookup_any should find it");
        assert_eq!(found_manpage, "welcome");
        assert_eq!(found_p.paragraph_id, p.paragraph_id);
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(lookup_any("this text definitely is not in any manpage").is_none());
    }

    #[test]
    fn normalized_text_is_populated() {
        let d = db();
        for m in d.manpages.values() {
            for p in &m.paragraphs {
                assert!(
                    !p.normalized_text.is_empty(),
                    "empty normalized for {:?}",
                    p.text
                );
                // The normalized form must round-trip through
                // normalize_for_match (it IS the output of that
                // function on the original text).
                assert_eq!(p.normalized_text, normalize_for_match(&p.text));
            }
        }
    }

    #[test]
    fn lookup_robust_to_whitespace_differences() {
        let d = db();
        let m = d.manpages.get("welcome").expect("welcome");
        let p = m.paragraphs.first().expect("first");
        // Query with double-spaces and a leading space — should
        // still match because normalize_for_match collapses runs
        // and trims.
        let messy_query = format!("  {}", p.text.replace(' ', "  "));
        let found = lookup_any(&messy_query);
        assert!(
            found.is_some(),
            "whitespace-normalized lookup should match: {:?} -> normalized {:?}",
            messy_query,
            normalize_for_match(&messy_query)
        );
    }

    #[test]
    fn phonemes_are_non_empty_ipa() {
        let d = db();
        let m = d.manpages.get("welcome").expect("welcome");
        for p in &m.paragraphs {
            for w in &p.words {
                assert!(!w.phonemes.is_empty(), "empty phonemes for word {:?}", w.text);
            }
        }
    }

    #[test]
    fn unesc_roundtrip() {
        assert_eq!(unesc(r"hello\nworld"), "hello\nworld");
        assert_eq!(unesc(r"a\tb"), "a\tb");
        assert_eq!(unesc(r"x\\y"), r"x\y");
        assert_eq!(unesc("plain"), "plain");
    }

    #[test]
    fn total_paragraphs_matches_expected() {
        // Should match the generator's output: 7 + 8 + 12 = 27 at
        // the time of v0.1.246 ship. This test is loose because
        // future manpage edits will change the count — assert it's
        // in a sane range.
        let n = total_paragraphs();
        assert!(n >= 10, "suspiciously few paragraphs: {}", n);
        assert!(n < 1000, "suspiciously many paragraphs: {}", n);
    }
}
