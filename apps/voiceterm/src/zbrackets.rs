//! v0.1.667: ZBrackets — Zettelkasten note linking (from a precursor
//! project). Write `[[name]]` in a note to link another note; it resolves to
//! a relative link, defaulting to .md (and hiding that extension), supporting
//! PDFs (ereader / old-device compat), and leaving numeric citations and
//! `-`-escaped brackets untouched.
//!
//! Semantics (cleaned up from the original; the original's two versions
//! disagreed, so these are stated explicitly):
//!   [[Name]]        → <a href="REL/Name.md">Name</a>   (.md default, hidden)
//!   [[Name.pdf]]    → <a href="REL/Name.pdf">Name.pdf</a> (ext kept, links a PDF)
//!   [[.Name]]       → smart: resolve, show only the basename (hide folder)
//!   [[,sub/Name]]   → show the folder in the display text
//!   [[-Name]]       → escaped: passthrough literal "[[Name]]" (don't link)
//!   [[12]]          → numeric & short → citation, passthrough "[[12]]"
//! Bare `Name` is resolved against an index (filename → path) built from the
//! notes tree; unresolved names link to the name as-typed (so a new note can
//! be created on click later).
//!
//! PURE + testable: parsing, path math, and rewriting are here; building the
//! index from the real notes dir + click handling are the wiring layer.

use std::collections::HashMap;

pub const DEFAULT_EXT: &str = "md";

/// Build a filename → path index from a list of note paths. A bare `[[Name]]`
/// resolves via this. Stores by basename (with and without extension) so
/// `[[Go]]` finds `.../Go.md`. Last write wins on collision (callers can
/// pre-sort for determinism).
pub fn build_index(paths: &[String]) -> HashMap<String, String> {
    let mut idx = HashMap::new();
    for p in paths {
        let base = p.rsplit('/').next().unwrap_or(p).to_string();
        idx.insert(base.clone(), p.clone());
        if let Some(stem) = base.rsplit_once('.').map(|(s, _)| s) {
            idx.insert(stem.to_string(), p.clone());
        }
    }
    idx
}

/// Relative path FROM one file's location TO another file. Ported from the
/// precursor's `path_difference` (the correct version), e.g.
/// (.../Notes/impakt/reVos.md, .../Notes/Go.md) → "../Go.md".
pub fn path_difference(from: &str, to: &str) -> String {
    let aa: Vec<&str> = from.split('/').collect();
    let bb: Vec<&str> = to.split('/').collect();
    let mut i = 0usize;
    // advance over the common prefix
    while i < aa.len() && i < bb.len() && aa[i] == bb[i] {
        i += 1;
    }
    // i is now the first differing index (or the end of the shorter path)
    let n_back = aa.len().saturating_sub(i).saturating_sub(1); // dirs to climb
    let fwd = bb[i..].join("/");
    let mut out = "../".repeat(n_back);
    out.push_str(&fwd);
    out
}

/// Split a filename stem and extension. Returns (stem, ext) where ext is the
/// DEFAULT if none present.
///
/// v0.1.668 (reasoned, not inherited): the precursor's recursive scan was
/// never battle-tested and would treat any trailing ".word" as an extension.
/// In a notes Zettelkasten, real extensions are SHORT (md, pdf, txt, png,
/// html, jpeg ≤4 chars), but dotted *titles* are common ("report.final",
/// "meeting.notes", "v1.2 plan"). So only treat a trailing token as an
/// extension when it's ≤4 chars, slash-free, and alphanumeric — otherwise
/// it's part of the name and we default to .md.
fn split_ext(name: &str) -> (String, String) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !ext.is_empty()
            && ext.len() <= 4
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
            => (stem.to_string(), ext.to_string()),
        _ => (name.to_string(), DEFAULT_EXT.to_string()),
    }
}

/// The result of classifying one `[[...]]` token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// leave the original `[[inner]]` literally (citation or `-`-escaped)
    Passthrough(String),
    /// a resolvable link: (href, display)
    Link { href: String, display: String },
}

/// Classify + resolve one bracket inner string (text between `[[` and `]]`),
/// given the current file's path and the name→path index.
pub fn resolve_token(inner: &str, current_path: &str, index: &HashMap<String, String>) -> Token {
    // v0.1.668: trim surrounding whitespace so "[[ Go ]]" resolves like
    // "[[Go]]" (the precursor didn't, and would seek a file literally named
    // " Go "). An all-whitespace/empty token is left literal.
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return Token::Passthrough(format!("[[{}]]", inner));
    }
    let inner = trimmed;
    // numeric short → citation, passthrough
    if inner.len() < 4 && inner.chars().all(|c| c.is_ascii_digit()) && !inner.is_empty() {
        return Token::Passthrough(format!("[[{}]]", inner));
    }
    // prefix handling
    let (mode, rest) = match inner.chars().next() {
        Some('-') => ('-', &inner[1..]),  // escaped
        Some('.') => ('.', &inner[1..]),  // smart: hide folder in display
        Some(',') => (',', &inner[1..]),  // show folder in display
        _ => (' ', inner),
    };
    if mode == '-' {
        return Token::Passthrough(format!("[[{}]]", rest));
    }
    if rest.is_empty() {
        return Token::Passthrough(format!("[[{}]]", inner));
    }

    let (stem, ext) = split_ext(rest);
    let file_name = format!("{}.{}", stem.rsplit('/').next().unwrap_or(&stem), ext);

    // resolve the target path: index lookup by basename, else by stem, else
    // treat `rest` as a path relative to the notes root (best effort).
    let target = index.get(&file_name)
        .or_else(|| index.get(stem.rsplit('/').next().unwrap_or(&stem)))
        .cloned()
        .unwrap_or_else(|| {
            // unresolved: link to rest as-typed under the current dir
            let dir = current_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            if dir.is_empty() { format!("{}.{}", stem, ext) }
            else { format!("{}/{}.{}", dir, stem, ext) }
        });

    let href = if current_path.is_empty() { target.clone() }
               else { path_difference(current_path, &target) };

    // display text: hide the default ext; honor folder-show/hide mode
    let shown_ext = if ext == DEFAULT_EXT { String::new() } else { format!(".{}", ext) };
    let display = match mode {
        '.' => format!("{}{}", stem.rsplit('/').next().unwrap_or(&stem), shown_ext), // basename only
        ',' => format!("{}{}", rest.trim_end_matches(&format!(".{}", ext)), shown_ext), // keep folder
        _   => format!("{}{}", stem, shown_ext),
    };
    Token::Link { href, display }
}

/// Rewrite all `[[...]]` tokens in `markdown` to resolved HTML links (or
/// passthrough for citations / escaped).
///
/// v0.1.668: code-aware. The precursor NOTED the need for code-block
/// detection but never implemented it — so a `[[x]]` inside a fenced code
/// block or inline `code` span would get wrongly rewritten. This processes
/// line-by-line: fenced blocks (``` or ~~~) are emitted verbatim, and within
/// a prose line, single-backtick inline-code spans are skipped. Links don't
/// span lines (a `[[` with no `]]` on the same line is left literal), which
/// is also safer than the precursor's whole-document split.
pub fn rewrite(markdown: &str, current_path: &str, index: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut in_fence = false;
    for line in markdown.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = body.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            // a fence delimiter toggles in/out of a fenced code block; emit
            // it (and everything inside) verbatim.
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        out.push_str(&rewrite_line(line, current_path, index));
    }
    out
}

/// Rewrite `[[...]]` in a single line, skipping single-backtick inline-code
/// spans. (Double-backtick spans — used when the content itself has a
/// backtick — aren't specially handled; that's rare in note prose.)
fn rewrite_line(line: &str, current_path: &str, index: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut in_inline_code = false;
    while i < line.len() {
        let b = bytes[i];
        if b == b'`' {
            in_inline_code = !in_inline_code;
            out.push('`');
            i += 1;
            continue;
        }
        // v0.1.732: check for `[[` at the BYTE level. The old code did
        // `&line[i..i+2] == "[["`, but when `i` indexes into a multibyte char
        // (e.g. a leading emoji like 🎬, bytes 0..4), `i+2` is not a char
        // boundary and the str slice PANICS. `[` is ASCII 0x5B, which never
        // appears as a continuation/lead byte of a multibyte UTF-8 sequence, so
        // a byte compare is both correct and panic-safe. (Reported: editing a
        // mindmap heading "🎬 TRANSFORMERS: DOMINION" crashed here.)
        if !in_inline_code && i + 1 < line.len() && b == b'[' && bytes[i + 1] == b'[' {
            if let Some(close) = line[i + 2..].find("]]") {
                let inner = &line[i + 2..i + 2 + close];
                match resolve_token(inner, current_path, index) {
                    Token::Passthrough(s) => out.push_str(&s),
                    Token::Link { href, display } =>
                        out.push_str(&format!("<a href=\"{}\">{}</a>", href, display)),
                }
                i = i + 2 + close + 2;
                continue;
            }
        }
        let ch_len = utf8_len(b);
        out.push_str(&line[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 { 1 } else if b < 0xE0 { 2 } else if b < 0xF0 { 3 } else { 4 }
}

/// v0.1.671: what an activated ZBracket link should DO. The report: rendered
/// `[[links]]` weren't integrated into the open-new-tab/doc menu, so clicking
/// one fell through to the teleprompter (the href text got read as prose
/// instead of opening anything). This classifies an href so the link-
/// activation handler can route it: a note → open in a (new) tab/pane, a PDF
/// → doc pane (ereader path), a URL → browser, an anchor → in-page jump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkAction {
    OpenNote(String),   // a .md (or extensionless) note path → note view / new tab
    OpenDoc(String),    // a .pdf/.epub/etc → doc pane
    OpenUrl(String),    // http(s):// → browser
    JumpAnchor(String), // #anchor → in-page
}

/// Classify an activated link href into the action it should trigger.
pub fn classify_link(href: &str) -> LinkAction {
    let h = href.trim();
    if let Some(anchor) = h.strip_prefix('#') {
        return LinkAction::JumpAnchor(anchor.to_string());
    }
    let low = h.to_lowercase();
    if low.starts_with("http://") || low.starts_with("https://")
        || low.starts_with("www.") {
        return LinkAction::OpenUrl(h.to_string());
    }
    // by extension: doc-pane formats vs note formats
    let ext = h.rsplit('.').next().filter(|e| *e != h).map(|e| e.to_lowercase());
    match ext.as_deref() {
        Some("pdf") | Some("epub") | Some("djvu") | Some("ps") | Some("mobi")
            => LinkAction::OpenDoc(h.to_string()),
        // .md or no extension → a note
        _ => LinkAction::OpenNote(h.to_string()),
    }
}
/// `[[...]]` inners (excluding numeric citations and `-`-escaped), each
/// normalized to its stem (no extension, no prefix char). Used to build the
/// note graph for adjacency-aware autocomplete. Code-fence aware (same rules
/// as `rewrite`).
pub fn link_targets(markdown: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in markdown.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = body.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence { continue; }
        // walk inline, skipping backtick code spans
        let bytes = line.as_bytes();
        let mut i = 0;
        let mut in_code = false;
        while i < line.len() {
            let b = bytes[i];
            if b == b'`' { in_code = !in_code; i += 1; continue; }
            // v0.1.732: byte-level `[[` check (see rewrite_line) — str-slicing
            // `&line[i..i+2]` panics when i is mid-multibyte-char.
            if !in_code && i + 1 < line.len() && b == b'[' && bytes[i + 1] == b'[' {
                if let Some(close) = line[i + 2..].find("]]") {
                    let inner = line[i + 2..i + 2 + close].trim();
                    // skip citations + escaped
                    let is_citation = inner.len() < 4 && !inner.is_empty()
                        && inner.chars().all(|c| c.is_ascii_digit());
                    if !is_citation && !inner.starts_with('-') && !inner.is_empty() {
                        // strip a leading . or , display-prefix, then ext
                        let rest = inner.trim_start_matches(['.', ',']);
                        let (stem, _ext) = split_ext(rest);
                        let base = stem.rsplit('/').next().unwrap_or(&stem).to_string();
                        if !base.is_empty() { out.push(base); }
                    }
                    i = i + 2 + close + 2;
                    continue;
                }
            }
            i += utf8_len(b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn idx(paths: &[&str]) -> HashMap<String, String> {
        build_index(&paths.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    // path_difference fixtures taken directly from the precursor's comments.
    #[test]
    fn path_difference_fixtures() {
        let root = "/Users/thekitchen/Desktop/kzn";
        assert_eq!(path_difference(
            &format!("{}/Notes/impakt/reVos.md", root),
            &format!("{}/Notes/Go.md", root)), "../Go.md");
        assert_eq!(path_difference(
            &format!("{}/Notes/impakt/design/reVos.md", root),
            &format!("{}/Notes/Go.md", root)), "../../Go.md");
        assert_eq!(path_difference(
            &format!("{}/Notes/Data Structure.md", root),
            &format!("{}/Notes/Go.md", root)), "Go.md");
        assert_eq!(path_difference(
            &format!("{}/README.md", root),
            &format!("{}/Notes/Go.md", root)), "Notes/Go.md");
        assert_eq!(path_difference(
            &format!("{}/Notes/rpi/backwork/ECSE-2410--Signals/Signals/HW/HW12/Assignment_12.pdf", root),
            &format!("{}/Notes/Go.md", root)),
            "../../../../../../Go.md");
    }

    #[test]
    fn standard_link_defaults_md_and_hides_ext() {
        let index = idx(&["/n/Notes/Go.md", "/n/Notes/impakt/reVos.md"]);
        let t = resolve_token("Go", "/n/Notes/impakt/reVos.md", &index);
        assert_eq!(t, Token::Link { href: "../Go.md".into(), display: "Go".into() });
    }

    #[test]
    fn pdf_link_keeps_ext() {
        let index = idx(&["/n/Notes/paper.pdf", "/n/Notes/a.md"]);
        let t = resolve_token("paper.pdf", "/n/Notes/a.md", &index);
        assert_eq!(t, Token::Link { href: "paper.pdf".into(), display: "paper.pdf".into() });
    }

    #[test]
    fn numeric_citation_passthrough() {
        let index = idx(&[]);
        assert_eq!(resolve_token("12", "/n/a.md", &index), Token::Passthrough("[[12]]".into()));
        assert_eq!(resolve_token("1", "/n/a.md", &index), Token::Passthrough("[[1]]".into()));
    }

    #[test]
    fn dash_escape_passthrough() {
        let index = idx(&[]);
        assert_eq!(resolve_token("-Name", "/n/a.md", &index), Token::Passthrough("[[Name]]".into()));
    }

    #[test]
    fn smart_prefix_hides_folder_in_display() {
        let index = idx(&["/n/Notes/sub/Deep.md", "/n/Notes/a.md"]);
        let t = resolve_token(".sub/Deep", "/n/Notes/a.md", &index);
        match t {
            Token::Link { href, display } => {
                assert_eq!(display, "Deep", "smart hides folder");
                assert_eq!(href, "sub/Deep.md");
            }
            _ => panic!("expected link"),
        }
    }

    #[test]
    fn unresolved_name_links_under_current_dir() {
        let index = idx(&["/n/Notes/a.md"]);
        let t = resolve_token("BrandNew", "/n/Notes/a.md", &index);
        match t {
            Token::Link { href, .. } => assert_eq!(href, "BrandNew.md"),
            _ => panic!("expected link"),
        }
    }

    #[test]
    fn rewrite_mixes_links_citations_and_text() {
        let index = idx(&["/n/Notes/HTML.md", "/n/Notes/a.md"]);
        let md = "See [[HTML]] and cite [[3]] but skip [[-Literal]] here.";
        let out = rewrite(md, "/n/Notes/a.md", &index);
        assert!(out.contains("<a href=\"HTML.md\">HTML</a>"));
        assert!(out.contains("[[3]]"), "citation kept");
        assert!(out.contains("[[Literal]]"), "escaped kept literal");
        assert!(!out.contains("[[HTML]]"), "link consumed");
    }

    #[test]
    fn rewrite_leaves_unclosed_brackets() {
        let index = idx(&[]);
        let md = "an [[unclosed token";
        assert_eq!(rewrite(md, "/n/a.md", &index), md);
    }

    // v0.1.732 (dogfood crash): a heading led by a multibyte char (emoji)
    // panicked at the `&line[i..i+2] == "[["` slice when i landed mid-emoji.
    #[test]
    fn rewrite_emoji_led_line_does_not_panic() {
        let index = idx(&["/n/Notes/HTML.md"]);
        // exact shape from the report + a real [[ link after the emoji.
        let md = "🎬 TRANSFORMERS: DOMINION see [[HTML]] for notes";
        let out = rewrite(md, "/n/Notes/a.md", &index);
        assert!(out.contains("🎬 TRANSFORMERS: DOMINION"), "emoji preserved");
        assert!(out.contains("<a href=\"HTML.md\">HTML</a>"), "link still resolves");
    }

    #[test]
    fn link_targets_emoji_led_line_does_not_panic() {
        // the second scanner (link_targets) had the same slice.
        let md = "🚀 launch notes [[Roadmap]] and 日本語 [[語]]";
        let t = link_targets(md);
        assert!(t.iter().any(|s| s == "Roadmap"));
        assert!(t.iter().any(|s| s == "語"));
    }

    #[test]
    fn rewrite_emoji_immediately_before_brackets() {
        // adjacency stress: emoji directly abutting [[ with no space.
        let index = idx(&["/n/Notes/X.md"]);
        let md = "🎬[[X]]🎬";
        let out = rewrite(md, "/n/Notes/a.md", &index);
        assert!(out.contains("<a href=\"X.md\">X</a>"));
        assert_eq!(out.matches('🎬').count(), 2);
    }

    // ── v0.1.668 reasoned-audit tests (not inherited from the precursor) ──

    #[test]
    fn dotted_titles_are_not_extensions() {
        // "report.final" / "meeting.notes" are TITLES, not files with a
        // "final"/"notes" extension — default to .md.
        let index = idx(&[]);
        for (name, want_href) in [("report.final", "report.final.md"),
                                  ("meeting.notes", "meeting.notes.md")] {
            match resolve_token(name, "/n/a.md", &index) {
                Token::Link { href, .. } => assert_eq!(href, want_href, "{}", name),
                _ => panic!("expected link for {}", name),
            }
        }
        // but real short extensions ARE honored
        match resolve_token("paper.pdf", "/n/a.md", &index) {
            Token::Link { href, .. } => assert_eq!(href, "paper.pdf"),
            _ => panic!("pdf should be a link"),
        }
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        let index = idx(&["/n/Notes/Go.md", "/n/Notes/a.md"]);
        let t = resolve_token(" Go ", "/n/Notes/a.md", &index);
        assert_eq!(t, Token::Link { href: "Go.md".into(), display: "Go".into() });
    }

    #[test]
    fn fenced_code_block_is_not_rewritten() {
        let index = idx(&["/n/HTML.md", "/n/a.md"]);
        let md = "prose [[HTML]]\n```\ncode [[HTML]] here\n```\nmore [[HTML]]\n";
        let out = rewrite(md, "/n/a.md", &index);
        // prose links rewritten...
        assert!(out.contains("<a href=\"HTML.md\">HTML</a>"));
        // ...but the one inside the fence is left literal
        assert!(out.contains("code [[HTML]] here"), "fenced [[HTML]] preserved");
    }

    #[test]
    fn inline_code_span_is_not_rewritten() {
        let index = idx(&["/n/HTML.md", "/n/a.md"]);
        let md = "use the `[[HTML]]` syntax to link [[HTML]]\n";
        let out = rewrite(md, "/n/a.md", &index);
        assert!(out.contains("`[[HTML]]`"), "inline-code [[HTML]] preserved literally");
        assert!(out.contains("<a href=\"HTML.md\">HTML</a>"), "the prose one is linked");
    }

    #[test]
    fn fence_with_language_tag_toggles() {
        let index = idx(&["/n/x.md", "/n/a.md"]);
        let md = "```rust\nlet x = \"[[x]]\";\n```\n[[x]]\n";
        let out = rewrite(md, "/n/a.md", &index);
        assert!(out.contains("\"[[x]]\""), "code with lang tag preserved");
        assert!(out.contains("<a href=\"x.md\">x</a>"), "post-fence prose linked");
    }

    #[test]
    fn link_targets_extracts_resolvable_only() {
        let md = "links [[HTML]] and [[notes/Deep]], cite [[3]], escape [[-Lit]],\n\
                  in code `[[Skip]]` and\n```\n[[AlsoSkip]]\n```\n";
        let mut t = link_targets(md);
        t.sort();
        assert_eq!(t, vec!["Deep".to_string(), "HTML".to_string()]);
    }

    #[test]
    fn classify_link_routes_by_kind() {
        // the fix for "links propagate the teleprompter": each href type
        // routes to a real action, not prose.
        assert_eq!(classify_link("../Go.md"), LinkAction::OpenNote("../Go.md".into()));
        assert_eq!(classify_link("Notes/Plan"), LinkAction::OpenNote("Notes/Plan".into()));
        assert_eq!(classify_link("paper.pdf"), LinkAction::OpenDoc("paper.pdf".into()));
        assert_eq!(classify_link("book.epub"), LinkAction::OpenDoc("book.epub".into()));
        assert_eq!(classify_link("https://example.com"),
                   LinkAction::OpenUrl("https://example.com".into()));
        assert_eq!(classify_link("#section-2"), LinkAction::JumpAnchor("section-2".into()));
    }
}
