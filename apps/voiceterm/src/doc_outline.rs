//! v0.1.695: document outline ("codemap") — extract a markdown document's
//! headings into a navigable list (level, title, line) for a codemap panel,
//! and fuzzy-match a spoken phrase to an entry so "jump to <section>" works
//! by voice. PURE + testable.
//!
//! Also surfaces fenced-code blocks and top-level list groups as minor entries
//! so the map is useful even for heading-light notes — but headings dominate.

#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem {
    /// heading level 1-6 (or 0 for a non-heading landmark like a code block)
    pub level: u8,
    pub title: String,
    /// 0-based line index in the document
    pub line: usize,
}

/// Build the outline. Headings (`#`..`######`) become items; a fenced code
/// block becomes a level-0 "‹code›" landmark (lang if given). ATX-closing
/// hashes are stripped. Setext headings (=== / --- underlines) are also
/// detected.
pub fn outline(doc: &str) -> Vec<OutlineItem> {
    let lines: Vec<&str> = doc.split('\n').collect();
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut fence_marker = "";
    for (i, raw) in lines.iter().enumerate() {
        let t = raw.trim_start();
        // fenced code toggling
        if t.starts_with("```") || t.starts_with("~~~") {
            let m = &t[..3];
            if !in_fence {
                in_fence = true;
                fence_marker = m;
                let lang = t[3..].trim();
                let title = if lang.is_empty() { "‹code›".to_string() }
                            else { format!("‹{}›", lang) };
                out.push(OutlineItem { level: 0, title, line: i });
            } else if t.starts_with(fence_marker) {
                in_fence = false;
            }
            continue;
        }
        if in_fence { continue; }

        // ATX heading
        let hashes = t.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
            let title = t[hashes..].trim()
                .trim_end_matches('#').trim().to_string();
            if !title.is_empty() {
                out.push(OutlineItem { level: hashes as u8, title, line: i });
            }
            continue;
        }
        // Setext heading: a non-empty line followed by === or --- underline
        if i + 1 < lines.len() && !t.is_empty() {
            let next = lines[i + 1].trim();
            if next.len() >= 2 && (next.chars().all(|c| c == '=')
                || next.chars().all(|c| c == '-')) {
                let level = if next.starts_with('=') { 1 } else { 2 };
                out.push(OutlineItem { level, title: t.trim().to_string(), line: i });
            }
        }
    }
    out
}

/// Wire for QML: "level\ttitle\tline" per item, '\n'-joined.
pub fn outline_to_wire(items: &[OutlineItem]) -> String {
    items.iter()
        .map(|it| format!("{}\t{}\t{}", it.level,
            it.title.replace(['\t', '\n'], " "), it.line))
        .collect::<Vec<_>>().join("\n")
}

/// Fuzzy-match a spoken query to the best outline item; returns its index.
/// Case-insensitive; scores by whole-phrase containment, then word overlap,
/// then subsequence. None if nothing plausibly matches.
pub fn jump_match(query: &str, items: &[OutlineItem]) -> Option<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() || items.is_empty() { return None; }
    let qwords: Vec<&str> = q.split_whitespace().collect();
    let mut best: Option<(usize, f32)> = None;
    for (idx, it) in items.iter().enumerate() {
        let title = it.title.to_lowercase();
        let mut score = 0.0f32;
        if title == q { score += 100.0; }
        if title.contains(&q) { score += 50.0; }
        // word overlap
        let twords: Vec<&str> = title.split_whitespace().collect();
        for qw in &qwords {
            if twords.iter().any(|tw| tw == qw) { score += 10.0; }
            else if twords.iter().any(|tw| tw.contains(qw)) { score += 4.0; }
        }
        // only a real content match counts; a heading then gets a small
        // tie-break bonus over code landmarks. No content match → no jump.
        if score > 0.0 {
            if it.level >= 1 { score += 1.0; }
            match best {
                Some((_, b)) if b >= score => {}
                _ => best = Some((idx, score)),
            }
        }
    }
    best.map(|(i, _)| i)
}

/// v0.1.696 (codemap live-follow): given the outline and the current 0-based
/// line, return the index of the section the line is "in" — the last heading
/// at or before the line. None if the line precedes the first heading.
pub fn active_section(items: &[OutlineItem], current_line: usize) -> Option<usize> {
    let mut active = None;
    for (i, it) in items.iter().enumerate() {
        // only real headings define sections (skip level-0 code landmarks)
        if it.level >= 1 && it.line <= current_line {
            active = Some(i);
        }
    }
    active
}

/// v0.1.701: the body text of the section starting at `heading_line` — from
/// that heading up to (not including) the next heading of the same-or-higher
/// level. For "read this section" aloud. Includes the heading title line.
pub fn section_text(doc: &str, heading_line: usize) -> String {
    let lines: Vec<&str> = doc.split('\n').collect();
    if heading_line >= lines.len() { return String::new(); }
    let items = outline(doc);
    // the level of the heading at heading_line (0 if it isn't a heading)
    let start_level = items.iter()
        .find(|it| it.line == heading_line && it.level >= 1)
        .map(|it| it.level).unwrap_or(0);
    // find the next heading at <= start_level after this line
    let mut end = lines.len();
    if start_level >= 1 {
        for it in &items {
            if it.line > heading_line && it.level >= 1 && it.level <= start_level {
                end = it.line;
                break;
            }
        }
    }
    lines[heading_line..end].join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# Title\n\nintro text\n\n## Setup\n\ninstall steps\n\n## Usage\n\n```rust\nfn main() {}\n```\n\n### Advanced\n\nmore";

    #[test]
    fn active_section_follows_line() {
        let o = outline(DOC);
        // DOC: # Title(0), ## Setup(4), ## Usage(8), code(10), ### Advanced(14)
        // line 1 → Title
        assert_eq!(o[active_section(&o, 1).unwrap()].title, "Title");
        // line 5 → Setup
        assert_eq!(o[active_section(&o, 5).unwrap()].title, "Setup");
        // line 9 → Usage (code landmark at 10 is skipped for sectioning)
        assert_eq!(o[active_section(&o, 11).unwrap()].title, "Usage");
        // line 20 → Advanced
        assert_eq!(o[active_section(&o, 20).unwrap()].title, "Advanced");
    }

    #[test]
    fn active_section_none_before_first_heading() {
        let o = outline("intro\n\n# First");
        assert!(active_section(&o, 0).is_none());
        assert_eq!(o[active_section(&o, 2).unwrap()].title, "First");
    }

    #[test]
    fn section_text_extracts_body() {
        // DOC: # Title(0) ... ## Setup(4) install steps ## Usage(8) ...
        let s = section_text(DOC, 4); // Setup
        assert!(s.starts_with("## Setup"));
        assert!(s.contains("install steps"));
        assert!(!s.contains("Usage"), "stops at the next ## heading");
    }

    #[test]
    fn section_text_higher_level_includes_subsections() {
        // # Title spans everything until the next h1 (none) → whole doc
        let d = "# Top\n\nintro\n\n## Sub\n\nbody\n\n# Next\n\nx";
        let s = section_text(d, 0);
        assert!(s.contains("Sub") && s.contains("body"));
        assert!(!s.contains("Next"), "stops at the next h1");
    }

    #[test]
    fn extracts_headings_with_lines() {
        let o = outline(DOC);
        let titles: Vec<&str> = o.iter().map(|i| i.title.as_str()).collect();
        assert!(titles.contains(&"Title"));
        assert!(titles.contains(&"Setup"));
        assert!(titles.contains(&"Usage"));
        assert!(titles.contains(&"Advanced"));
        // levels
        let title = o.iter().find(|i| i.title == "Title").unwrap();
        assert_eq!(title.level, 1);
        let adv = o.iter().find(|i| i.title == "Advanced").unwrap();
        assert_eq!(adv.level, 3);
    }

    #[test]
    fn code_block_is_landmark() {
        let o = outline(DOC);
        let code = o.iter().find(|i| i.title.contains("rust"));
        assert!(code.is_some(), "fenced code shows as a landmark");
        assert_eq!(code.unwrap().level, 0);
    }

    #[test]
    fn headings_inside_fence_ignored() {
        let d = "# Real\n\n```\n# not a heading\n```\n\n## Also Real";
        let o = outline(d);
        let titles: Vec<&str> = o.iter().filter(|i| i.level >= 1)
            .map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["Real", "Also Real"]);
    }

    #[test]
    fn setext_headings() {
        let d = "Big Title\n=========\n\nSub\n---\n\nbody";
        let o = outline(d);
        assert_eq!(o[0].title, "Big Title");
        assert_eq!(o[0].level, 1);
        assert_eq!(o[1].title, "Sub");
        assert_eq!(o[1].level, 2);
    }

    #[test]
    fn atx_closing_hashes_stripped() {
        let o = outline("## Setup ##\n");
        assert_eq!(o[0].title, "Setup");
    }

    #[test]
    fn voice_jump_matches_section() {
        let o = outline(DOC);
        // exact
        let i = jump_match("usage", &o).unwrap();
        assert_eq!(o[i].title, "Usage");
        // partial word
        let j = jump_match("the advanced part", &o).unwrap();
        assert_eq!(o[j].title, "Advanced");
        // no match
        assert!(jump_match("nonexistent xyzzy", &o).is_none());
    }

    #[test]
    fn wire_round_trip() {
        let o = outline("# A\n## B");
        let w = outline_to_wire(&o);
        assert!(w.contains("1\tA\t0"));
        assert!(w.contains("2\tB\t1"));
    }
}
