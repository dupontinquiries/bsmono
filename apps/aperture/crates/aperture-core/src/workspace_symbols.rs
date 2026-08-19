//! workspace_symbols — "go to symbol in workspace" (Sublime/Pulsar's
//! symbols-view, `Ctrl+T`): the per-file [`crate::syntax::Syntax::outline`]
//! already used for the single-file symbol popup and the AI context filler,
//! run over every file in a walked candidate list instead of just the open
//! one, so a name search can jump across files, not only within one.
//!
//! This is a *symbol index*, not semantic go-to-definition: two `fn new`s in
//! different modules both show up, and there's no reference resolution.
//! That's an honest scope line, not a stopgap — real go-to-def needs either
//! an LSP (already named, unbuilt, in TODO.md) or a name-resolution pass
//! neither tree-sitter's syntax tree nor this crate's `ts.rs` seam attempts.
//! A name index still answers "where is `fn parse_line`" far faster than
//! `Ctrl+Shift+F` for the common case (the name is distinctive), at zero
//! additional machinery: it's `outline()`, called once per file instead of
//! once per open.
//!
//! Rust-only today, same as the rest of the tree-sitter seam (`Language::
//! from_path` maps only `.rs` to `Language::Rust`; everything else stays
//! `Language::Plain`, whose backend's `outline()` is deliberately empty —
//! see `syntax.rs`'s doc comment on why a guessed outline is worse than
//! none). Extending this to another language is "give it a grammar", not a
//! change here.

use crate::syntax::{Language, OutlineItemAlias as OutlineItem, Syntax};
use crate::Buffer;
use std::path::{Path, PathBuf};

/// One named place in the workspace: an outline item plus which file it
/// came from. `item.line`/`item.kind`/`item.name`/`item.sig` are the same
/// fields the single-file symbol popup already reads.
#[derive(Clone, Debug)]
pub struct Symbol {
    pub item: OutlineItem,
    pub path: PathBuf,
    /// Repo-relative display path — same convention as `file_finder::rank`
    /// and `project_search::Hit::rel`, so all three pickers read alike.
    pub rel: String,
}

/// Parse every Rust file in `files` and collect their outlines into one
/// flat index, capped at `max_files` (a parse-per-file is real CPU work,
/// unlike `project_search`'s plain string scan — the cap bounds a `Ctrl+T`
/// press in a huge repo the same way `file_finder::walk_files`'s cap bounds
/// `Ctrl+O`). Files that fail to read, or aren't Rust, are skipped, not
/// errors: a workspace symbol index that stops at the first unreadable
/// generated file isn't one.
pub fn index_files(root: &Path, files: &[PathBuf], max_files: usize) -> Vec<Symbol> {
    let mut out = Vec::new();
    for f in files.iter().take(max_files) {
        if Language::from_path(f) != Language::Rust {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let buf = Buffer::from_str(&text);
        let mut syntax = Syntax::new(Language::Rust);
        let rel = f.strip_prefix(root).unwrap_or(f).to_string_lossy().into_owned();
        for item in syntax.outline(&buf) {
            out.push(Symbol { item, path: f.clone(), rel: rel.clone() });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aperture-workspace-symbols-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn indexes_symbols_across_multiple_files() {
        let root = tmp("multi");
        std::fs::write(root.join("a.rs"), "fn alpha() {}\n").unwrap();
        std::fs::write(root.join("b.rs"), "struct Beta;\n").unwrap();
        let files = vec![root.join("a.rs"), root.join("b.rs")];

        let symbols = index_files(&root, &files, 100);
        let names: Vec<&str> = symbols.iter().map(|s| s.item.name.as_str()).collect();
        assert!(names.contains(&"alpha"), "{names:?}");
        assert!(names.contains(&"Beta"), "{names:?}");
        let alpha = symbols.iter().find(|s| s.item.name == "alpha").unwrap();
        assert_eq!(alpha.rel, "a.rs");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn non_rust_files_are_skipped_without_erroring() {
        let root = tmp("non-rust");
        std::fs::write(root.join("notes.txt"), "fn looks_like_rust() {}\n").unwrap();
        let files = vec![root.join("notes.txt")];
        assert!(index_files(&root, &files, 100).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn max_files_cap_is_respected() {
        let root = tmp("cap");
        let files: Vec<PathBuf> = (0..5)
            .map(|i| {
                let p = root.join(format!("f{i}.rs"));
                std::fs::write(&p, format!("fn item_{i}() {{}}\n")).unwrap();
                p
            })
            .collect();

        let symbols = index_files(&root, &files, 2);
        assert_eq!(symbols.len(), 2, "stops parsing after max_files, not after max symbols");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn impl_methods_are_indexed_as_type_dot_method_same_as_the_single_file_outline() {
        let root = tmp("impl-methods");
        std::fs::write(
            root.join("a.rs"),
            "struct Widget;\nimpl Widget {\n    fn build(&self) {}\n}\n",
        )
        .unwrap();
        let files = vec![root.join("a.rs")];

        let symbols = index_files(&root, &files, 100);
        let names: Vec<&str> = symbols.iter().map(|s| s.item.name.as_str()).collect();
        assert!(names.contains(&"Widget::build"), "{names:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
