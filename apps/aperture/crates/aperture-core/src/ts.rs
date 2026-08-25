//! ts — the tree-sitter backend behind the syntax seam (feature `tree-sitter`).
//!
//! Design, in one breath: the buffer records every mutation as a [`TextEdit`]
//! (bytes + points, tree-sitter's native shape); we replay those into
//! `Tree::edit` and reparse **with the old tree**, which is what makes edits
//! O(damage) instead of O(file); highlight spans come from running the
//! grammar's own highlights query over just the wanted line's byte range,
//! resolved per byte (later captures win, matching the query's
//! most-specific-last convention) and memoized per line until the next edit.
//!
//! Deliberately NOT here (v0): injections (Rust-in-markdown etc.), locals
//! tracking, and the tree-sitter-highlight crate that provides both — a
//! direct `QueryCursor` is ~a tenth the surface and all a single-language
//! editor needs. The seam stays: this module is one arm of `syntax::Backend`.
//!
//! Version notes (the churn that costs time):
//! - tree-sitter 0.25 returns `QueryCaptures` as a `streaming_iterator::
//!   StreamingIterator` — `while let Some(..) = it.next()`, not a for loop.
//! - Grammar crates bridge through `tree-sitter-language`, so runtime and
//!   grammar version-skew *compiles*; bump them together anyway.
//! - `Point.column` is a BYTE column. Lapce ships a wrong column here
//!   (distance to next line start) without visible damage — evidence
//!   tree-sitter leans on byte offsets — but we compute it correctly.

use crate::buffer::{Buffer, TextEdit};
use crate::syntax::{Language, Span, TokKind};
use std::collections::HashMap;
use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Parser, Point, Query, QueryCursor, TextProvider, Tree};

pub struct TsSyntax {
    parser: Parser,
    tree: Option<Tree>,
    query: Query,
    /// Capture index → span kind, resolved once at query build.
    kinds: Vec<TokKind>,
    /// Per-line span memo; cleared on every reparse.
    lines: HashMap<usize, Vec<Span>>,
}

/// Longest-prefix capture-name mapping (the lapce/helix scope convention:
/// names are dotted paths, `keyword.control.import` is still a keyword).
fn kind_for(name: &str) -> TokKind {
    let head = name.split('.').next().unwrap_or(name);
    match head {
        "keyword" => TokKind::Keyword,
        "string" | "char" => TokKind::Str,
        "comment" => TokKind::Comment,
        "number" | "float" | "integer" => TokKind::Number,
        "constant" => {
            if name.contains("numeric") {
                TokKind::Number
            } else {
                TokKind::Plain
            }
        }
        "attribute" | "derive" => TokKind::Attr,
        "function" | "method" | "constructor" => TokKind::Fn,
        "type" | "struct" | "enum" | "trait" => TokKind::Type,
        "escape" => TokKind::Str,
        _ => TokKind::Plain,
    }
}

impl TsSyntax {
    /// `None` when the language has no grammar here — caller falls back to
    /// the hand lexer. Only Rust ships in v0 (the seam takes more).
    pub fn new(lang: Language) -> Option<TsSyntax> {
        let ts_lang: tree_sitter::Language = match lang {
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
            _ => return None,
        };
        let mut parser = Parser::new();
        parser.set_language(&ts_lang).ok()?;
        let query = Query::new(&ts_lang, tree_sitter_rust::HIGHLIGHTS_QUERY).ok()?;
        let kinds = query.capture_names().iter().map(|n| kind_for(n)).collect();
        Some(TsSyntax { parser, tree: None, query, kinds, lines: HashMap::new() })
    }

    fn reparse(&mut self, buf: &Buffer) {
        let old = self.tree.take();
        self.tree = self.parser.parse_with_options(
            &mut |byte, _pos: Point| -> &[u8] {
                if byte >= buf.len_bytes() {
                    return &[];
                }
                let (chunk, start) = buf.chunk_at_byte(byte);
                &chunk.as_bytes()[byte - start..]
            },
            old.as_ref(),
            None,
        );
        self.lines.clear();
    }

    /// Replay recorded buffer edits into the old tree, then reparse
    /// incrementally. With no prior tree (first call), just parse.
    pub fn apply_edits(&mut self, buf: &Buffer, edits: &[TextEdit]) {
        if let Some(tree) = &mut self.tree {
            for e in edits {
                tree.edit(&InputEdit {
                    start_byte: e.start_byte,
                    old_end_byte: e.old_end_byte,
                    new_end_byte: e.new_end_byte,
                    start_position: Point::new(e.start_point.0, e.start_point.1),
                    old_end_position: Point::new(e.old_end_point.0, e.old_end_point.1),
                    new_end_position: Point::new(e.new_end_point.0, e.new_end_point.1),
                });
            }
            if edits.is_empty() {
                return; // nothing changed; keep the memo too
            }
        }
        self.reparse(buf);
    }

    /// Spans for one line, in line-byte coordinates (the renderer's shape).
    pub fn spans_for(&mut self, line: usize, buf: &Buffer) -> Vec<Span> {
        if self.tree.is_none() {
            self.reparse(buf);
        }
        if let Some(hit) = self.lines.get(&line) {
            return hit.clone();
        }
        let Some(tree) = &self.tree else { return Vec::new() };
        let start = buf.line_to_byte(line);
        let raw_end = buf.line_to_byte(line + 1);
        if raw_end <= start {
            return Vec::new();
        }
        // EXCLUDE THE LINE TERMINATOR. `line_to_byte(line + 1)` is the start
        // of the NEXT line, so the span it bounds includes this line's "\n"
        // (and "\r\n"), but the renderer slices `buffer.line(line)`, which
        // does not. Emitting spans in the wider coordinate space made
        // `&text[s.range]` index one (or two) bytes past the end of a
        // fully-highlighted line and panic — "end byte index 43 is out of
        // bounds for string of length 42", on opening any file whose first
        // visible line ends in a highlighted token.
        //
        // Trimmed HERE, at the one place the two coordinate systems meet,
        // rather than clamped at each of the renderer's slice sites: the
        // spans are cached and handed to anything that asks, so a span that
        // is wrong by construction would have to be defended against
        // everywhere it is read.
        let end = start + buf.line(line).len();
        if end <= start {
            return Vec::new();
        }
        // Per-byte resolution: later captures overwrite earlier ones, which is
        // how highlights queries express specificity (generic rules first,
        // specific overrides after). Lines are short; O(len) is fine.
        let mut per_byte = vec![TokKind::Plain; end - start];
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(start..end);
        let provider = BufProvider(buf);
        let mut caps = cursor.captures(&self.query, tree.root_node(), provider);
        while let Some((m, ci)) = caps.next() {
            let cap = m.captures[*ci];
            let kind = self.kinds[cap.index as usize];
            if kind == TokKind::Plain {
                continue;
            }
            let r = cap.node.byte_range();
            let (a, b) = (r.start.max(start) - start, r.end.min(end) - start);
            for slot in &mut per_byte[a..b] {
                *slot = kind;
            }
        }
        let mut spans = Vec::new();
        let mut i = 0;
        while i < per_byte.len() {
            let k = per_byte[i];
            let mut j = i + 1;
            while j < per_byte.len() && per_byte[j] == k {
                j += 1;
            }
            if k != TokKind::Plain {
                spans.push(Span { range: i..j, kind: k });
            }
            i = j;
        }
        self.lines.insert(line, spans.clone());
        spans
    }
}

/// Query text predicates (`#eq?`, `#match?`) need node text; feed rope chunks
/// without materializing. tree-sitter 0.25's TextProvider is generic over the
/// chunk item, so `&str` chunks slot straight in.
struct BufProvider<'a>(&'a Buffer);

impl<'a> TextProvider<&'a [u8]> for BufProvider<'a> {
    type I = std::iter::Map<
        Box<dyn Iterator<Item = &'a str> + 'a>,
        fn(&'a str) -> &'a [u8],
    >;
    fn text(&mut self, node: tree_sitter::Node) -> Self::I {
        let it: Box<dyn Iterator<Item = &'a str> + 'a> =
            Box::new(self.0.byte_slice_chunks(node.byte_range()));
        it.map(str::as_bytes as fn(&'a str) -> &'a [u8])
    }
}

// ─── outline: the file as a list of named places ────────────────────────────
// Shared engine for two consumers with the same need from opposite ends:
// the FIM context filler (the model reads it) and the physical rooms (the
// human walks it). Two levels only — top-level items, plus methods inside
// impls as "Type::method" — because an outline is a MAP, and a map that
// shows every closet is a floor plan.

/// What a station in the file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineKind {
    Fn,
    Struct,
    Enum,
    Trait,
    Impl,
    Mod,
    Const,
}

#[derive(Clone, Debug)]
pub struct OutlineItem {
    pub kind: OutlineKind,
    /// `name` for items, `Type::method` for methods.
    pub name: String,
    /// First line, trimmed — the signature as a human reads it.
    pub sig: String,
    pub line: usize,
    pub end_line: usize,
}

impl TsSyntax {
    pub fn outline(&mut self, buf: &Buffer) -> Vec<OutlineItem> {
        if self.tree.is_none() {
            self.reparse(buf);
        }
        let Some(tree) = &self.tree else { return Vec::new() };
        let mut out = Vec::new();
        let mut walk = tree.root_node().walk();
        for node in tree.root_node().children(&mut walk) {
            let Some((kind, name)) = item_of(node, buf) else { continue };
            push_item(&mut out, kind, name.clone(), node, buf);
            if kind == OutlineKind::Impl {
                let mut w2 = node.walk();
                for ch in node.children(&mut w2) {
                    if ch.kind() != "declaration_list" {
                        continue;
                    }
                    let mut w3 = ch.walk();
                    for m in ch.children(&mut w3) {
                        if m.kind() == "function_item" {
                            if let Some(mn) = field_text(m, "name", buf) {
                                push_item(&mut out, OutlineKind::Fn, format!("{name}::{mn}"), m, buf);
                            }
                        }
                    }
                }
            }
        }
        out
    }
}

fn push_item(out: &mut Vec<OutlineItem>, kind: OutlineKind, name: String, node: tree_sitter::Node, buf: &Buffer) {
    let line = node.start_position().row;
    let sig = buf.line(line).trim().trim_end_matches('{').trim_end().to_string();
    out.push(OutlineItem { kind, name, sig, line, end_line: node.end_position().row });
}

fn field_text(node: tree_sitter::Node, field: &str, buf: &Buffer) -> Option<String> {
    let n = node.child_by_field_name(field)?;
    Some(buf.byte_slice_chunks(n.byte_range()).collect::<String>())
}

fn item_of(node: tree_sitter::Node, buf: &Buffer) -> Option<(OutlineKind, String)> {
    let kind = match node.kind() {
        "function_item" => OutlineKind::Fn,
        "struct_item" => OutlineKind::Struct,
        "enum_item" => OutlineKind::Enum,
        "trait_item" => OutlineKind::Trait,
        "impl_item" => OutlineKind::Impl,
        "mod_item" => OutlineKind::Mod,
        "const_item" | "static_item" => OutlineKind::Const,
        _ => return None,
    };
    let name = if kind == OutlineKind::Impl {
        field_text(node, "type", buf)?
    } else {
        field_text(node, "name", buf)?
    };
    Some((kind, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans_text(ts: &mut TsSyntax, buf: &Buffer, line: usize) -> Vec<(String, TokKind)> {
        let text = buf.line(line).into_owned();
        ts.spans_for(line, buf)
            .into_iter()
            .map(|s| (text[s.range.clone()].to_string(), s.kind))
            .collect()
    }

    #[test]
    fn rust_gets_real_grammar_kinds() {
        let buf = Buffer::from_str("fn main() {\n    let x = \"hi\"; // note\n}\n");
        let mut ts = TsSyntax::new(Language::Rust).expect("rust grammar");
        let l0 = spans_text(&mut ts, &buf, 0);
        assert!(l0.iter().any(|(t, k)| t == "fn" && *k == TokKind::Keyword), "{l0:?}");
        assert!(l0.iter().any(|(t, k)| t == "main" && *k == TokKind::Fn), "the WIN over the lexer: {l0:?}");
        let l1 = spans_text(&mut ts, &buf, 1);
        assert!(l1.iter().any(|(t, k)| t == "\"hi\"" && *k == TokKind::Str), "{l1:?}");
        assert!(l1.iter().any(|(t, k)| t.starts_with("//") && *k == TokKind::Comment), "{l1:?}");
    }

    #[test]
    fn incremental_edit_updates_only_whats_stale() {
        let mut buf = Buffer::from_str("fn a() {}\nfn b() {}\n");
        let mut ts = TsSyntax::new(Language::Rust).unwrap();
        assert!(spans_text(&mut ts, &buf, 1).iter().any(|(t, k)| t == "b" && *k == TokKind::Fn));
        // Turn line 0 into a comment; line 1 must survive untouched.
        let at = 0;
        buf.insert(at, "// ");
        let edits = buf.drain_edits();
        ts.apply_edits(&buf, &edits);
        let l0 = spans_text(&mut ts, &buf, 0);
        assert!(l0.iter().all(|(_, k)| *k == TokKind::Comment), "whole line commented: {l0:?}");
        assert!(spans_text(&mut ts, &buf, 1).iter().any(|(t, k)| t == "b" && *k == TokKind::Fn));
    }

    #[test]
    fn multibyte_edits_keep_points_honest() {
        // 'é' is 2 bytes: a wrong char-vs-byte column here corrupts the tree's
        // point bookkeeping on the NEXT incremental edit.
        let mut buf = Buffer::from_str("// é comment\nfn x() {}\n");
        let mut ts = TsSyntax::new(Language::Rust).unwrap();
        let _ = ts.spans_for(1, &buf);
        let ins_at = buf.pos_to_char(crate::buffer::Pos::new(1, 3));
        buf.insert(ins_at, "y");
        let edits = buf.drain_edits();
        ts.apply_edits(&buf, &edits);
        let l1 = spans_text(&mut ts, &buf, 1);
        assert!(l1.iter().any(|(t, k)| t == "yx" && *k == TokKind::Fn), "{l1:?}");
    }

    #[test]
    fn unsupported_language_declines() {
        assert!(TsSyntax::new(Language::Plain).is_none());
    }
    #[test]
    fn outline_maps_the_file_two_levels_deep() {
        let buf = Buffer::from_str(
            "mod util;\npub struct Cfg { n: u32 }\nimpl Cfg {\n    pub fn load() -> Cfg { todo!() }\n    fn tweak(&mut self) {}\n}\nfn main() {\n    let c = Cfg::load();\n}\n",
        );
        let mut ts = TsSyntax::new(Language::Rust).unwrap();
        let o = ts.outline(&buf);
        let names: Vec<(&str, OutlineKind)> = o.iter().map(|i| (i.name.as_str(), i.kind)).collect();
        assert_eq!(
            names,
            vec![
                ("util", OutlineKind::Mod),
                ("Cfg", OutlineKind::Struct),
                ("Cfg", OutlineKind::Impl),
                ("Cfg::load", OutlineKind::Fn),
                ("Cfg::tweak", OutlineKind::Fn),
                ("main", OutlineKind::Fn),
            ],
            "{o:#?}"
        );
        let main = o.last().unwrap();
        assert_eq!((main.line, main.end_line), (6, 8), "ranges cover bodies");
        assert_eq!(o[1].sig, "pub struct Cfg n: u32 }".replace(" n", " { n"), "sig is the trimmed first line");
    }

    #[test]
    fn spans_never_reach_past_the_rendered_line() {
        // THE OPEN-A-FILE CRASH. Spans were bounded by
        // `line_to_byte(line + 1)` — the start of the NEXT line, which
        // includes this line's newline — while the renderer slices
        // `buffer.line(line)`, which excludes it. A line whose last token
        // is highlighted therefore produced a span ending one byte past
        // what the renderer indexes:
        //   "end byte index 43 is out of bounds for string of length 42"
        // Every line of a real file goes through this, so the guarantee is
        // checked over the whole buffer rather than on one hand-picked line.
        // Lines ENDING in a highlighted token are the trigger: the span
        // runs to the line's last byte, so a one-past bound is one past
        // the rendered text. A trailing `;` or `{` is Plain and hides it,
        // which is why the original 4-line sample did not reproduce.
        // A line-comment runs to end-of-line, and tree-sitter's comment
        // node INCLUDES the newline in its byte range — that is the case
        // where `.min(end)` with the untrimmed bound yields a span one
        // past the rendered text. This is the shape that actually crashed
        // (aperture opening any commented file), so it is the fixture.
        let src = include_str!("syntax.rs"); // a real 900-line file, not a toy
        let buf = Buffer::from_str(src);
        let mut ts = TsSyntax::new(Language::Rust).expect("rust grammar");
        for line in 0..buf.len_lines() {
            let text = buf.line(line).into_owned();
            for sp in ts.spans_for(line, &buf) {
                assert!(
                    sp.range.end <= text.len(),
                    "line {line} ({:?}, {} bytes) got span {:?} — this is the panic",
                    text, text.len(), sp.range
                );
                // And it must be sliceable, not merely in range: a span
                // ending mid-UTF-8 would panic just as hard.
                let _ = &text[sp.range.clone()];
            }
        }
    }
}
