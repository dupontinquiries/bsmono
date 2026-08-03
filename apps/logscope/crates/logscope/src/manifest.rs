//! manifest.rs — what the thinker sees before it decides which files to open.
//!
//! `docroute` narrows lexically and cannot judge intent: it knows `arena_place` is rare, not that the
//! question is about memory. The thinker can judge intent and cannot read 800 files. So the split is
//! **lexical narrows, the model chooses** — and what it chooses from is a manifest.
//!
//! ## Why the last line is not the last line
//!
//! The obvious manifest entry is path plus first and last line. In Rust the last line is `}` in almost
//! every file, which tells a reader nothing and costs a row in the budget. The last MEANINGFUL line —
//! skipping closing delimiters and blanks — is usually the end of the final impl or a test name, which
//! actually says where the file finishes.
//!
//! Same for the first: `//!` module docs are the single most informative line a Rust file has, and
//! taking line 1 literally would often take a `use` statement instead.
//!
//! ## The manifest has a budget too
//!
//! A repo of 800 files at 100 characters each is 80 KB of manifest, which is most of a small model's
//! window before a single file is opened. So the manifest is built from a lexically-narrowed set, not
//! from the tree.

/// One row the thinker reads.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub path: String,
    /// The most informative opening line — a module doc if there is one, else the first non-trivial
    /// line.
    pub opens: String,
    /// The last line that is not a closing delimiter or blank.
    pub ends: String,
    /// Cost of opening this file, so the model can budget rather than guess.
    pub tokens: usize,
}

impl Entry {
    /// One manifest row, capped so a long line cannot crowd out the rest of the list.
    pub fn render(&self, width: usize) -> String {
        let clip = |s: &str| -> String {
            let t = s.trim();
            if t.chars().count() <= width { t.to_string() }
            else { t.chars().take(width.saturating_sub(1)).collect::<String>() + "…" }
        };
        format!("{} [{}t]\n  ┌ {}\n  └ {}", self.path, self.tokens, clip(&self.opens), clip(&self.ends))
    }
}

/// Is this line worth showing?
///
/// A bare `}` or `);` is structure, not content, and a manifest full of them describes nothing.
fn meaningful(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() { return false; }
    !t.chars().all(|c| matches!(c, '}' | ')' | ']' | ';' | ',' | '{'))
}

/// Build a manifest row for one file.
///
/// `tokens` is the caller's estimate; this does not guess at tokenization.
pub fn entry(path: &str, body: &str, tokens: usize) -> Entry {
    let lines: Vec<&str> = body.lines().collect();
    // A module doc says what the file is FOR, which is the question the thinker is answering. Prefer
    // it over line 1, which is as likely to be a `use` as anything.
    let opens = lines.iter()
        .take(20)
        .find(|l| l.trim_start().starts_with("//!") || l.trim_start().starts_with("\"\"\""))
        .map(|l| l.trim_start().trim_start_matches("//!").trim().to_string())
        .or_else(|| lines.iter().find(|l| meaningful(l)).map(|l| l.trim().to_string()))
        .unwrap_or_default();
    let ends = lines.iter().rev().find(|l| meaningful(l))
        .map(|l| l.trim().to_string()).unwrap_or_default();
    Entry { path: path.to_string(), opens, ends, tokens }
}

/// Render a manifest that fits `budget_chars`.
///
/// Truncating the LIST is better than truncating the rows: a model can ask about a file it can see
/// named, and cannot ask about one that was silently dropped — so the count of omitted files is
/// stated rather than hidden.
pub fn render(entries: &[Entry], width: usize, budget_chars: usize) -> String {
    let mut out = String::new();
    let mut shown = 0usize;
    for e in entries {
        let row = e.render(width);
        if out.len() + row.len() + 1 > budget_chars { break; }
        out.push_str(&row);
        out.push('\n');
        shown += 1;
    }
    if shown < entries.len() {
        out.push_str(&format!("… and {} more files not shown — narrow the query or raise the budget\n",
            entries.len() - shown));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUSTY: &str = "//! arena.rs — slab allocator for resident weights.\nuse std::sync::Mutex;\n\npub fn arena_place() {\n    let x = 1;\n}\n";

    #[test]
    fn the_MODULE_DOC_is_preferred_over_line_one() {
        // It says what the file is FOR, which is the question the thinker is answering. Line 1 is as
        // likely to be a `use` as anything.
        let e = entry("arena.rs", RUSTY, 100);
        assert!(e.opens.starts_with("arena.rs — slab allocator"), "{}", e.opens);
    }

    #[test]
    fn the_last_line_is_the_last_MEANINGFUL_one() {
        // Almost every Rust file ends in `}`. A manifest of closing braces describes nothing and costs
        // a row per file.
        let e = entry("arena.rs", RUSTY, 100);
        assert_eq!(e.ends, "let x = 1;", "got {:?}", e.ends);
        assert_ne!(e.ends, "}");
    }

    #[test]
    fn a_file_with_no_doc_falls_back_to_its_first_real_line() {
        let e = entry("x.rs", "\n\nuse std::io;\nfn main() {}\n", 10);
        assert_eq!(e.opens, "use std::io;");
    }

    #[test]
    fn an_empty_file_yields_empty_strings_rather_than_panicking() {
        let e = entry("empty.rs", "", 0);
        assert!(e.opens.is_empty() && e.ends.is_empty());
    }

    #[test]
    fn a_long_line_is_CLIPPED_so_it_cannot_crowd_out_the_list() {
        let long = "//! ".to_string() + &"x".repeat(500);
        let e = entry("l.rs", &long, 10);
        let r = e.render(60);
        assert!(r.lines().all(|l| l.chars().count() <= 66), "{r}");
        assert!(r.contains('…'));
    }

    #[test]
    fn the_TOKEN_COST_is_shown_so_the_model_can_budget() {
        // Otherwise it picks by name and discovers the size after committing the window.
        assert!(entry("a.rs", RUSTY, 4200).render(80).contains("[4200t]"));
    }

    #[test]
    fn omitted_files_are_COUNTED_not_silently_dropped() {
        // A model can ask about a file it can see named and cannot ask about one that vanished, so a
        // truncated list must say it was truncated.
        let es: Vec<Entry> = (0..50).map(|i| entry(&format!("f{i}.rs"), RUSTY, 10)).collect();
        let r = render(&es, 60, 400);
        assert!(r.contains("more files not shown"), "{r}");
        assert!(r.contains("narrow the query"), "and says what to do about it");
    }

    #[test]
    fn a_manifest_that_FITS_says_nothing_about_omissions() {
        let es: Vec<Entry> = (0..2).map(|i| entry(&format!("f{i}.rs"), RUSTY, 10)).collect();
        assert!(!render(&es, 60, 10_000).contains("not shown"));
    }
}
