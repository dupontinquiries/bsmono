//! v0.1.723: backlinks — the inverse of the [[wiki-link]] graph. Given the
//! link sets of every known note, build a reverse index so the editor can show
//! "what links here" for the current note. Pure: the fs scan + [[ ]] extraction
//! that feeds it lives in main.rs; this module is the parsing + indexing core.

use std::collections::BTreeMap;

/// Extract the link TARGETS from a note body: every `[[target]]` (the part
/// before an optional `|alias` or `#anchor`), lowercased + trimmed, de-duped in
/// order. Handles `[[a|b]]` (target a), `[[a#sec]]` (target a), nested brackets
/// defensively (takes the inner-most close).
pub fn extract_targets(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            // find the closing ]]
            if let Some(close) = body[i + 2..].find("]]") {
                let inner = &body[i + 2..i + 2 + close];
                let target = normalize_target(inner);
                if !target.is_empty() && !out.contains(&target) {
                    out.push(target);
                }
                i += 2 + close + 2;
                continue;
            }
        }
        // advance by one char (utf8-safe)
        i += 1;
        while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 { i += 1; }
    }
    out
}

/// Normalize a raw `[[...]]` inner string to a comparable target key: strip an
/// `|alias`, strip a `#anchor`, trim, lowercase. ".md" suffix removed so
/// `[[foo]]` and `[[foo.md]]` collapse.
pub fn normalize_target(inner: &str) -> String {
    let mut s = inner.trim();
    if let Some(bar) = s.find('|') { s = &s[..bar]; }
    if let Some(hash) = s.find('#') { s = &s[..hash]; }
    let s = s.trim();
    let s = s.strip_suffix(".md").unwrap_or(s);
    s.trim().to_lowercase()
}

/// The note name (key) for a path: file stem, lowercased. `notes/Foo Bar.md`
/// → `foo bar`.
pub fn note_key(path: &str) -> String {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let stem = base.strip_suffix(".md").unwrap_or(base);
    stem.trim().to_lowercase()
}

/// A reverse index: target-note-key → the list of (path, display) notes that
/// link to it. Built from (path, body) pairs.
#[derive(Debug, Default)]
pub struct Backlinks {
    map: BTreeMap<String, Vec<(String, String)>>,
}

impl Backlinks {
    /// Build from an iterator of (path, body). Each note's [[targets]] add an
    /// edge target←source. A note never backlinks to itself.
    pub fn build(notes: &[(String, String)]) -> Self {
        let mut map: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for (path, body) in notes {
            let src_key = note_key(path);
            let display = display_name(path);
            for target in extract_targets(body) {
                if target == src_key { continue; } // no self-link
                let entry = map.entry(target).or_default();
                if !entry.iter().any(|(p, _)| p == path) {
                    entry.push((path.clone(), display.clone()));
                }
            }
        }
        // stable display order
        for v in map.values_mut() { v.sort_by(|a, b| a.1.cmp(&b.1)); }
        Backlinks { map }
    }

    /// Notes linking to `note_path` (matched by its key). (path, display) pairs.
    pub fn for_note(&self, note_path: &str) -> Vec<(String, String)> {
        self.map.get(&note_key(note_path)).cloned().unwrap_or_default()
    }

    /// Count of backlinks to a note (cheap badge).
    pub fn count_for(&self, note_path: &str) -> usize {
        self.map.get(&note_key(note_path)).map(|v| v.len()).unwrap_or(0)
    }

    /// Orphans: known notes that nothing links to (helps zettelkasten hygiene).
    /// `all_paths` is the full note set.
    pub fn orphans<'a>(&self, all_paths: &'a [String]) -> Vec<&'a String> {
        all_paths.iter()
            .filter(|p| !self.map.contains_key(&note_key(p)))
            .collect()
    }
}

/// Human display: the file stem (no dir, no .md), original case.
pub fn display_name(path: &str) -> String {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    base.strip_suffix(".md").unwrap_or(base).to_string()
}

/// Render backlinks to a wire for QML: "path\tdisplay" per line.
pub fn to_wire(links: &[(String, String)]) -> String {
    links.iter().map(|(p, d)| format!("{}\t{}", p, d)).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_basic_and_aliased() {
        let body = "see [[Index]] and [[foo|the foo note]] plus [[bar#section]].";
        let t = extract_targets(body);
        assert_eq!(t, vec!["index", "foo", "bar"]);
    }

    #[test]
    fn dedups_targets() {
        let t = extract_targets("[[a]] [[a]] [[A]] [[a.md]]");
        assert_eq!(t, vec!["a"]);
    }

    #[test]
    fn handles_no_links() {
        assert!(extract_targets("plain text, no links here").is_empty());
        assert!(extract_targets("[single] and ] [ stray").is_empty());
    }

    #[test]
    fn builds_reverse_index() {
        let notes = vec![
            ("notes/a.md".to_string(), "links to [[b]] and [[c]]".to_string()),
            ("notes/b.md".to_string(), "links to [[c]]".to_string()),
            ("notes/c.md".to_string(), "no outgoing links".to_string()),
        ];
        let bl = Backlinks::build(&notes);
        // c is linked from a and b
        let c = bl.for_note("notes/c.md");
        assert_eq!(c.len(), 2);
        assert!(c.iter().any(|(p, _)| p == "notes/a.md"));
        assert!(c.iter().any(|(p, _)| p == "notes/b.md"));
        // b is linked from a only
        assert_eq!(bl.count_for("notes/b.md"), 1);
        // a has no backlinks
        assert_eq!(bl.count_for("notes/a.md"), 0);
    }

    #[test]
    fn no_self_link() {
        let notes = vec![("notes/loop.md".to_string(), "I mention [[loop]] myself".to_string())];
        let bl = Backlinks::build(&notes);
        assert_eq!(bl.count_for("notes/loop.md"), 0);
    }

    #[test]
    fn orphans_found() {
        let notes = vec![
            ("a.md".to_string(), "[[b]]".to_string()),
            ("b.md".to_string(), "nothing".to_string()),
            ("c.md".to_string(), "nothing".to_string()),
        ];
        let bl = Backlinks::build(&notes);
        let all = vec!["a.md".to_string(), "b.md".to_string(), "c.md".to_string()];
        let orphans = bl.orphans(&all);
        // b is linked (not orphan); a and c are orphans
        assert!(orphans.iter().any(|p| p.as_str() == "a.md"));
        assert!(orphans.iter().any(|p| p.as_str() == "c.md"));
        assert!(!orphans.iter().any(|p| p.as_str() == "b.md"));
    }

    #[test]
    fn wire_roundtrip() {
        let links = vec![("notes/a.md".to_string(), "a".to_string())];
        assert_eq!(to_wire(&links), "notes/a.md\ta");
    }

    #[test]
    fn unicode_safe() {
        // multibyte chars around a link must not panic the byte scanner
        let t = extract_targets("café [[naïve]] résumé 日本 [[語]]");
        assert_eq!(t, vec!["naïve", "語"]);
    }
}
