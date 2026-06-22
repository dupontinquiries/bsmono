//! Apply a "lazy" partial edit onto a full file (pure, no deps) — the stitch behind the
//! "merge this as a commit/branch?" suggestion (cards.rs `MergeEdit`). Models emit edits like:
//!
//! ```text
//! // file: src/main.rs
//! pub enum CardinalGesture { Tap, Cardinal(Direction), Hairpin(Direction), NEW_Rotation, ... }
//! // ... existing code ...
//! impl CardinalThresholds { pub fn lenient() -> Self { ... } }
//! // ... rest of the code ...
//! ```
//!
//! i.e. concrete item blocks separated by elision markers that mean "keep the original here". This
//! merges item-by-item: a top-level item the edit redefines REPLACES the original's same-named item;
//! a new item is appended; an `impl T` block merges its inner fns into the original `impl T` (same
//! fn replaced, new fn appended, untouched fns kept). Everything not mentioned is left intact.
//!
//! Deliberately pragmatic (brace/line scanning, not a full parser): handles the common Rust item
//! forms the edit convention produces. Anything it can't confidently place is reported, not guessed.

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: String,      // "fn" | "struct" | "enum" | "impl" | "trait" | "mod" | "const" | "use" | …
    pub name: String,      // item name; for impl, the type (e.g. "CardinalThresholds")
    pub text: String,      // exact source of the item (no leading blank lines)
}

#[derive(Debug, Default, PartialEq)]
pub struct Stitch {
    pub merged: String,
    pub replaced: Vec<String>,  // "kind name" of items whose original was overwritten
    pub inserted: Vec<String>,  // new items appended
    pub impl_merged: Vec<String>, // "impl T" blocks whose inner fns were merged
    pub unplaced: Vec<String>,    // edit chunks we couldn't confidently place (left for the human)
}

const ELISION: &[&str] = &["existing code", "rest of the code", "rest of code", "unchanged", "...", "snip"];

fn is_elision(line: &str) -> bool {
    let l = line.trim();
    if !(l.starts_with("//") || l.starts_with("#") || l.starts_with("/*")) { return false; }
    let body = l.trim_start_matches(['/', '#', '*', ' ']).to_lowercase();
    body.contains("...") || ELISION.iter().any(|e| body.contains(e))
}
fn is_file_header(line: &str) -> bool {
    let l = line.trim().to_lowercase();
    (l.starts_with("//") || l.starts_with("#")) && l.contains("file:")
}

const KEYWORDS: &[&str] = &["fn","struct","enum","impl","trait","mod","const","static","type","use","macro_rules"];

/// First item keyword on a line (after visibility/attribute noise), if the line starts an item.
fn item_kind(line: &str) -> Option<&'static str> {
    let mut s = line.trim_start();
    // strip a leading `pub`, `pub(...)`, `async`, `unsafe`, `default`, `const ` qualifiers that precede fn/etc.
    loop {
        let before = s;
        for q in ["pub(crate)","pub(super)","pub","async","unsafe","default","extern \"C\"","extern"] {
            if let Some(r) = s.strip_prefix(q) { if r.starts_with([' ','(']) || r.is_empty() { s = r.trim_start(); } }
        }
        if s == before { break; }
    }
    for &k in KEYWORDS {
        if s == k || s.starts_with(&format!("{k} ")) || s.starts_with(&format!("{k}<")) || s.starts_with(&format!("{k}!")) {
            return Some(k);
        }
    }
    None
}

/// Pull the item's name. For `impl [<G>] Type [for Trait]` returns the implemented Type. For others,
/// the identifier after the keyword.
fn item_name(kind: &str, sig: &str) -> String {
    let after = sig.trim_start();
    if kind == "impl" {
        // skip "impl", optional generics, optional "Trait for"; take the type token.
        let mut rest = after.trim_start_matches("impl").trim_start();
        if rest.starts_with('<') { if let Some(i) = matching_angle(rest) { rest = rest[i + 1..].trim_start(); } }
        // `Trait for Type` → take after " for "
        if let Some(p) = rest.find(" for ") { rest = rest[p + 5..].trim_start(); }
        return rest.split([' ', '<', '{', '\n', '(']).next().unwrap_or("").to_string();
    }
    let kw_stripped = after.splitn(2, kind).nth(1).unwrap_or("").trim_start();
    kw_stripped.trim_start_matches('!').split([' ', '<', '(', '{', ';', ':', '\n']).find(|s| !s.is_empty()).unwrap_or("").to_string()
}

fn matching_angle(s: &str) -> Option<usize> { // index of the '>' matching the leading '<'
    let b = s.as_bytes(); let mut depth = 0i32;
    for (i, &c) in b.iter().enumerate() {
        if c == b'<' { depth += 1; } else if c == b'>' { depth -= 1; if depth == 0 { return Some(i); } }
    }
    None
}

/// Split source into top-level items (brace-balanced) and the gaps between them. Returns items in
/// order; non-item lines (comments/blank) attach to the following item as a prefix is NOT done here —
/// we keep items minimal so replacement is clean. Public so the artifact extractor can use it to tell
/// an additive continuation from a redefinition.
pub fn scan_items(src: &str) -> Vec<Item> {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some(kind) = item_kind(lines[i]) {
            let start = i;
            let (end, _) = capture_item(&lines, i);
            let text = lines[start..=end].join("\n");
            let sig = lines[start];
            items.push(Item { kind: kind.to_string(), name: item_name(kind, sig), text });
            i = end + 1;
        } else { i += 1; }
    }
    items
}

/// From the item start line, find its last line: brace-balanced for `{…}` items, or the line ending
/// in `;` for use/const/type/static without a body.
fn capture_item(lines: &[&str], start: usize) -> (usize, bool) {
    // does this item have a brace body? look until the first '{' or ';' at the statement level.
    let mut depth = 0i32; let mut saw_brace = false;
    for (off, line) in lines[start..].iter().enumerate() {
        for c in line.chars() {
            match c { '{' => { depth += 1; saw_brace = true; } '}' => { depth -= 1; } _ => {} }
        }
        if saw_brace && depth <= 0 { return (start + off, true); }
        if !saw_brace && line.trim_end().ends_with(';') { return (start + off, false); }
    }
    (lines.len() - 1, saw_brace)
}

/// Inner fns of an `impl` block (one level deep), as (name, text).
fn impl_fns(impl_text: &str) -> Vec<Item> {
    // strip the outer `impl … {` line and the trailing `}`.
    let first_brace = impl_text.find('{').map(|b| b + 1).unwrap_or(0);
    let last_brace = impl_text.rfind('}').unwrap_or(impl_text.len());
    if first_brace >= last_brace { return Vec::new(); }
    scan_items(&impl_text[first_brace..last_brace])
}

fn replace_impl_fn(impl_text: &str, fns_from_edit: &[Item]) -> (String, Vec<String>, Vec<String>) {
    let mut body = impl_text.to_string();
    let mut replaced = Vec::new(); let mut inserted = Vec::new();
    for f in fns_from_edit {
        if f.kind != "fn" { continue; }
        let existing = impl_fns(&body).into_iter().find(|x| x.kind == "fn" && x.name == f.name);
        match existing {
            Some(ex) => { body = body.replacen(&ex.text, &f.text, 1); replaced.push(format!("fn {}", f.name)); }
            None => {
                // insert before the final closing brace
                if let Some(pos) = body.rfind('}') {
                    let indented = f.text.lines().map(|l| if l.is_empty() { String::new() } else { format!("    {l}") }).collect::<Vec<_>>().join("\n");
                    body.insert_str(pos, &format!("{indented}\n"));
                    inserted.push(format!("fn {}", f.name));
                }
            }
        }
    }
    (body, replaced, inserted)
}

/// Split a partial edit into concrete item-bearing chunks, dropping elision markers + `// file:`
/// headers. Each returned string is a run of source between markers.
fn edit_chunks(partial: &str) -> Vec<String> {
    let mut chunks = Vec::new(); let mut cur = String::new();
    for line in partial.lines() {
        if is_elision(line) || is_file_header(line) {
            if !cur.trim().is_empty() { chunks.push(std::mem::take(&mut cur)); } else { cur.clear(); }
        } else { cur.push_str(line); cur.push('\n'); }
    }
    if !cur.trim().is_empty() { chunks.push(cur); }
    chunks
}

/// Apply `partial` (a lazy edit) onto `original`. See module docs.
pub fn stitch_partial(original: &str, partial: &str) -> Stitch {
    let mut out = original.to_string();
    let mut st = Stitch::default();
    let mut edit_items: Vec<Item> = Vec::new();
    for chunk in edit_chunks(partial) {
        let items = scan_items(&chunk);
        if items.is_empty() && !chunk.trim().is_empty() {
            st.unplaced.push(chunk.trim().lines().next().unwrap_or("").to_string());
        }
        edit_items.extend(items);
    }
    for it in &edit_items {
        let orig_items = scan_items(&out);
        if it.kind == "impl" {
            if let Some(orig) = orig_items.iter().find(|o| o.kind == "impl" && o.name == it.name) {
                let (merged, repl, ins) = replace_impl_fn(&orig.text, &impl_fns(&it.text));
                out = out.replacen(&orig.text, &merged, 1);
                if !repl.is_empty() || !ins.is_empty() { st.impl_merged.push(format!("impl {}", it.name)); }
                st.replaced.extend(repl.into_iter().map(|r| format!("{}::{r}", it.name)));
                st.inserted.extend(ins.into_iter().map(|r| format!("{}::{r}", it.name)));
                continue;
            }
        }
        match orig_items.iter().find(|o| o.kind == it.kind && o.name == it.name && !o.name.is_empty()) {
            Some(orig) => { out = out.replacen(&orig.text, &it.text, 1); st.replaced.push(format!("{} {}", it.kind, it.name)); }
            None => {
                let trimmed = out.trim_end();
                out = format!("{trimmed}\n\n{}\n", it.text);
                st.inserted.push(format!("{} {}", it.kind, it.name));
            }
        }
    }
    st.merged = out;
    st
}

/// A one-line commit-message summary of what the stitch did.
pub fn summary(st: &Stitch) -> String {
    let mut parts = Vec::new();
    if !st.replaced.is_empty() { parts.push(format!("replace {}", st.replaced.join(", "))); }
    if !st.inserted.is_empty() { parts.push(format!("add {}", st.inserted.join(", "))); }
    if parts.is_empty() { "no changes".into() } else { parts.join("; ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: &str = "use crate::gestures::SwipePoint;\n\
\n\
pub enum CardinalGesture {\n\
    Tap,\n\
    Cardinal(Direction),\n\
    Unrecognized,\n\
}\n\
\n\
impl CardinalThresholds {\n\
    pub fn strict() -> Self {\n\
        Self { tap_radius_px: 12.0 }\n\
    }\n\
    pub fn lenient() -> Self {\n\
        Self { tap_radius_px: 18.0 }\n\
    }\n\
}\n\
\n\
fn classify(p: Path) -> CardinalGesture {\n\
    CardinalGesture::Tap\n\
}\n";

    // a lazy edit: new enum variant, a rewritten lenient(), elision markers, a trailing usage snippet.
    const PARTIAL: &str = "// file: src/main.rs\n\
pub enum CardinalGesture {\n\
    Tap,\n\
    Cardinal(Direction),\n\
    Hairpin(Direction),\n\
    Unrecognized,\n\
}\n\
// ... existing code ...\n\
impl CardinalThresholds {\n\
    pub fn lenient() -> Self {\n\
        Self { tap_radius_px: 26.0 }\n\
    }\n\
}\n\
// ... rest of the code ...\n";

    #[test]
    fn scans_top_level_items() {
        let items = scan_items(ORIGINAL);
        let kinds: Vec<_> = items.iter().map(|i| (i.kind.as_str(), i.name.as_str())).collect();
        assert!(kinds.contains(&("use", "crate")) || kinds.iter().any(|(k, _)| *k == "use"), "found use");
        assert!(kinds.contains(&("enum", "CardinalGesture")));
        assert!(kinds.contains(&("impl", "CardinalThresholds")));
        assert!(kinds.contains(&("fn", "classify")));
    }

    #[test]
    fn replaces_enum_keeps_other_items() {
        let st = stitch_partial(ORIGINAL, PARTIAL);
        assert!(st.merged.contains("Hairpin(Direction)"), "new variant applied");
        assert!(st.merged.contains("fn classify"), "untouched fn kept");
        assert!(st.replaced.contains(&"enum CardinalGesture".to_string()), "enum reported replaced: {:?}", st.replaced);
    }

    #[test]
    fn merges_impl_fn_keeping_siblings() {
        let st = stitch_partial(ORIGINAL, PARTIAL);
        assert!(st.merged.contains("tap_radius_px: 26.0"), "lenient() rewritten");
        assert!(st.merged.contains("fn strict"), "sibling fn strict() kept inside the impl");
        assert!(st.merged.matches("fn lenient").count() == 1, "lenient not duplicated");
    }

    #[test]
    fn new_item_is_inserted_not_lost() {
        let edit = "// ... existing code ...\nfn brand_new() -> u8 { 7 }\n";
        let st = stitch_partial(ORIGINAL, edit);
        assert!(st.merged.contains("fn brand_new"), "new fn inserted");
        assert!(st.inserted.contains(&"fn brand_new".to_string()));
        assert!(st.merged.contains("fn classify"), "original preserved");
    }

    #[test]
    fn usage_snippet_without_items_is_reported_not_dropped() {
        // the trailing "let thresholds = …" snippet has no top-level item → unplaceable, must be flagged.
        let edit = "// file: src/main.rs\n// In your main:\nlet thresholds = CardinalThresholds::lenient();\n";
        let st = stitch_partial(ORIGINAL, edit);
        assert!(!st.unplaced.is_empty(), "loose snippet should be reported as unplaced, not silently dropped");
        assert_eq!(st.merged, ORIGINAL, "nothing item-level → original unchanged");
    }

    #[test]
    fn summary_reads_like_a_commit() {
        let st = stitch_partial(ORIGINAL, PARTIAL);
        let s = summary(&st);
        assert!(s.contains("CardinalGesture"), "summary mentions the change: {s}");
    }
}
