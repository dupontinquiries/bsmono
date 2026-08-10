//! astfind — find a Rust item across the monorepo, read its doc, see who calls it.
//!
//! ## Why not `syn`
//!
//! A real parse is more correct and it is the wrong trade here. `syn` with
//! `full` is a heavy build dependency for a dev tool, it fails on a file that
//! does not compile — which is exactly when you are searching for something —
//! and it needs the whole file in memory. This scans line-wise, keeps what it
//! recognises, and is explicit about what it cannot see.
//!
//! **What it deliberately does not do:** resolve `use` aliases, distinguish two
//! `new` methods on different types, or understand macros. Those need type
//! information, and a tool that pretended otherwise would be confidently wrong
//! about the one thing you asked it.
//!
//! ## Doc comments are the payload, not a nicety
//!
//! This monorepo puts the reasoning in `//!` and `///` — why a threshold is 40,
//! why a bonus cancels at 8 characters. Finding a function without its doc
//! finds half of it, so docs are indexed as a first-class field and searchable.

pub mod score;

/// What kind of item was found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { Fn, Struct, Enum, Trait, Impl, Mod, Const, Type }

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Fn => "fn", Kind::Struct => "struct", Kind::Enum => "enum",
            Kind::Trait => "trait", Kind::Impl => "impl", Kind::Mod => "mod",
            Kind::Const => "const", Kind::Type => "type",
        }
    }
}

/// One indexed item.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub name: String,
    pub kind: Kind,
    pub file: String,
    /// 1-based, so it can be handed to an editor unchanged.
    pub line: usize,
    pub is_pub: bool,
    /// The `///` block immediately above, with markers stripped.
    pub doc: String,
    /// The signature line, trimmed.
    pub signature: String,
}

impl Item {
    /// The first sentence of the doc, for a one-line preview.
    ///
    /// First *sentence* rather than first line: this codebase's docs open with
    /// a summary line, and truncating at a fixed width cuts it mid-word.
    pub fn summary(&self) -> String {
        let first = self.doc.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        match first.find(". ") {
            Some(i) => first[..=i].trim().to_string(),
            None => first.to_string(),
        }
    }

    /// What a picker matches against: the name, the path, and the doc.
    ///
    /// Including the doc means "why is the threshold 40" finds the item that
    /// explains it, which is the search this codebase actually rewards.
    pub fn haystack(&self) -> String {
        format!("{} {} {}", self.name, self.file, self.doc)
    }
}

/// An item doc — `///` only.
///
/// `//!` is deliberately NOT included. It documents the enclosing module, and
/// treating it as an item doc attaches the whole module header to whichever
/// item happens to come first — which in this codebase is a paragraph of
/// reasoning about something else entirely. Caught by a test asserting the
/// summary of `fn new`, which came back as the module's opening line.
fn strip_doc_line(l: &str) -> Option<&str> {
    let t = l.trim_start();
    t.strip_prefix("///").map(|s| s.trim_start())
}

/// A module-level doc line, kept separately.
fn strip_module_doc(l: &str) -> Option<&str> {
    l.trim_start().strip_prefix("//!").map(|s| s.trim_start())
}

/// Parse one file into items.
pub fn index_file(path: &str, source: &str) -> Vec<Item> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    let mut doc: Vec<String> = Vec::new();

    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim();

        if let Some(d) = strip_doc_line(raw) {
            doc.push(d.to_string());
            continue;
        }
        if strip_module_doc(raw).is_some() {
            // Not an item doc, and not a doc-clearing line either — a module
            // header sits above the first item with a blank line between.
            continue;
        }
        // Attributes sit between a doc block and its item, so they must not
        // clear the doc — otherwise every #[derive]'d struct loses its
        // documentation, which is most of them.
        if line.starts_with('#') { continue; }
        if line.is_empty() { continue; }

        // `pub`, `pub(crate)`, `pub(super)`, `pub(in path)`. The restricted
        // forms have NO SPACE after `pub`, so a `starts_with("pub ")` check
        // misses them entirely — the item is then not recognised at all,
        // because the rest of the line still begins with `pub(...)`. Found by a
        // test looking for a `pub(crate) const`.
        let (is_pub, body) = if let Some(r) = line.strip_prefix("pub(") {
            (true, r.split_once(')').map(|(_, t)| t.trim_start()).unwrap_or(r))
        } else if let Some(r) = line.strip_prefix("pub ") {
            (true, r)
        } else {
            (false, line)
        };

        let kind = if body.starts_with("fn ") || body.starts_with("async fn ")
            || body.starts_with("const fn ") || body.starts_with("unsafe fn ") { Some(Kind::Fn) }
            else if body.starts_with("struct ") { Some(Kind::Struct) }
            else if body.starts_with("enum ") { Some(Kind::Enum) }
            else if body.starts_with("trait ") { Some(Kind::Trait) }
            else if body.starts_with("impl ") { Some(Kind::Impl) }
            else if body.starts_with("mod ") { Some(Kind::Mod) }
            else if body.starts_with("const ") || body.starts_with("static ") { Some(Kind::Const) }
            else if body.starts_with("type ") { Some(Kind::Type) }
            else { None };

        if let Some(kind) = kind {
            if let Some(name) = item_name(body, kind) {
                out.push(Item {
                    name, kind, file: path.to_string(), line: i + 1, is_pub,
                    doc: doc.join("\n"), signature: line.to_string(),
                });
            }
        }
        doc.clear();
    }
    out
}

fn item_name(body: &str, kind: Kind) -> Option<String> {
    let rest = match kind {
        Kind::Fn => body.split_once("fn ")?.1,
        Kind::Impl => {
            // `impl Trait for Type` indexes as the Type: that is what somebody
            // searching for a type expects to land on.
            let r = body.strip_prefix("impl ")?;
            let r = r.split_once(" for ").map(|(_, t)| t).unwrap_or(r);
            r
        }
        _ => body.split_once(' ')?.1,
    };
    let name: String = rest.trim_start_matches('<').trim()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() { None } else { Some(name) }
}

/// A place a name is used.
#[derive(Clone, Debug, PartialEq)]
pub struct CallSite {
    pub file: String,
    pub line: usize,
    pub text: String,
}

/// Find call sites of `name`.
///
/// Word-boundary matched, so searching `new` does not return every `renew` —
/// and comments are skipped, because a name discussed in prose is not a caller
/// and mixing the two makes the count meaningless.
pub fn find_calls(name: &str, path: &str, source: &str) -> Vec<CallSite> {
    let mut out = Vec::new();
    for (i, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with("//") { continue; }
        let mut from = 0;
        while let Some(rel) = raw[from..].find(name) {
            let at = from + rel;
            let before = raw[..at].chars().last();
            let after = raw[at + name.len()..].chars().next();
            let bounded = before.map_or(true, |c| !(c.is_alphanumeric() || c == '_'))
                && after.map_or(true, |c| !(c.is_alphanumeric() || c == '_'));
            if bounded {
                out.push(CallSite { file: path.into(), line: i + 1, text: line.to_string() });
                break;
            }
            from = at + name.len();
        }
    }
    out
}

/// Rank items against a query.
pub fn search<'a>(items: &'a [Item], query: &str, limit: usize) -> Vec<(&'a Item, i32)> {
    let mut hits: Vec<(&Item, i32)> = items.iter()
        .filter_map(|it| score::score(query, &it.haystack()).map(|s| (it, s)))
        .collect();
    // Score, then file and line, so an unchanged index returns an unchanged
    // order — a picker that reshuffles equal-scoring rows between keystrokes is
    // one you select the wrong item from.
    hits.sort_by(|a, b| b.1.cmp(&a.1)
        .then_with(|| a.0.file.cmp(&b.0.file))
        .then_with(|| a.0.line.cmp(&b.0.line)));
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
//! Module docs are not an item.

/// Opens a panel for a risk score.
///
/// Below 40 the panel takes a tap. Above it, a hold.
pub fn new(score: u32) -> ChevronConfirm {
    let x = new_inner();
    renew(x)
}

/// One indexed thing.
#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
}

impl Display for Item {
    fn fmt(&self) -> Result { Ok(()) }
}

enum Kind { Fn }

pub(crate) const HOLD_MS: f32 = 1200.0;
"#;

    fn items() -> Vec<Item> { index_file("src/demo.rs", SRC) }

    #[test]
    fn functions_structs_enums_impls_and_consts_are_all_found() {
        let got = items();
        let names: Vec<(&str, Kind)> = got.iter().map(|i| (i.name.as_str(), i.kind)).collect();
        assert!(names.contains(&("new", Kind::Fn)));
        assert!(names.contains(&("Item", Kind::Struct)));
        assert!(names.contains(&("Kind", Kind::Enum)));
        assert!(names.contains(&("HOLD_MS", Kind::Const)));
    }

    #[test]
    fn an_attribute_between_a_doc_and_its_item_does_not_lose_the_doc() {
        // #[derive] sits between them, and clearing the doc on an attribute
        // would strip documentation from most structs in this repo.
        let s = items();
        let item = s.iter().find(|i| i.name == "Item").expect("Item");
        assert!(item.doc.contains("One indexed thing"), "doc was {:?}", item.doc);
    }

    #[test]
    fn an_impl_block_indexes_under_the_type_not_the_trait() {
        // Somebody searching for a type expects to land on its impl.
        let s = items();
        let im = s.iter().find(|i| i.kind == Kind::Impl).expect("impl");
        assert_eq!(im.name, "Item");
    }

    #[test]
    fn visibility_is_recorded_including_pub_crate() {
        let s = items();
        assert!(s.iter().find(|i| i.name == "new").unwrap().is_pub);
        assert!(!s.iter().find(|i| i.name == "Kind").unwrap().is_pub);
        assert!(s.iter().find(|i| i.name == "HOLD_MS").unwrap().is_pub, "pub(crate) is pub");
    }

    #[test]
    fn line_numbers_are_one_based_so_an_editor_can_take_them_unchanged() {
        let s = items();
        let f = s.iter().find(|i| i.name == "new").unwrap();
        assert_eq!(SRC.lines().nth(f.line - 1).unwrap().trim(), f.signature);
    }

    #[test]
    fn the_summary_is_the_first_sentence_not_a_truncation() {
        // This codebase opens docs with a summary line; cutting at a fixed
        // width lands mid-word.
        let s = items();
        let f = s.iter().find(|i| i.name == "new").unwrap();
        assert_eq!(f.summary(), "Opens a panel for a risk score.");
    }

    #[test]
    fn module_docs_do_not_become_an_item() {
        assert!(!items().iter().any(|i| i.name.contains("Module")));
    }

    #[test]
    fn searching_matches_the_doc_as_well_as_the_name() {
        // "why is the threshold 40" should find the item that explains it —
        // the search this codebase actually rewards.
        let s = items();
        let hits = search(&s, "risk score", 5);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].0.name, "new");
    }

    #[test]
    fn call_sites_respect_word_boundaries() {
        // Searching `new` must not return every `renew`, or the count means
        // nothing.
        let calls = find_calls("new", "src/demo.rs", SRC);
        let texts: Vec<&str> = calls.iter().map(|c| c.text.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("pub fn new")));
        assert!(!texts.iter().any(|t| t.contains("renew(x)")),
                "renew is not a call to new: {texts:?}");
    }

    #[test]
    fn call_sites_skip_comments() {
        // A name discussed in prose is not a caller, and mixing the two makes
        // the count meaningless.
        let src = "// we should call new() here one day\nlet a = new();";
        let calls = find_calls("new", "f.rs", src);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].line, 2);
    }

    #[test]
    fn one_line_reports_one_call_site_rather_than_several() {
        // Otherwise a line calling a helper twice doubles its apparent
        // popularity.
        let calls = find_calls("new", "f.rs", "let a = new(new(1));");
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn results_are_stable_between_identical_searches() {
        // A picker that reshuffles equal-scoring rows between keystrokes is one
        // you select the wrong item from.
        let s = items();
        let a: Vec<&str> = search(&s, "n", 20).iter().map(|(i, _)| i.name.as_str()).collect();
        let b: Vec<&str> = search(&s, "n", 20).iter().map(|(i, _)| i.name.as_str()).collect();
        assert_eq!(a, b);
    }

    #[test]
    fn a_file_that_does_not_compile_still_indexes() {
        // The moment you most want to search is when something is broken —
        // which is the whole argument against a real parser here.
        let broken = "/// Doc\npub fn half_written(a: u32 -> {\n";
        let got = index_file("broken.rs", broken);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "half_written");
    }
}
