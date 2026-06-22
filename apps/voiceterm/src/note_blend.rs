//! v0.1.683: editor blend rules — the "solid rules" for the integrated
//! buffer's live replacement + subrendering (user request: "a slightly more
//! integrated buffer with live replacement and subrendering ... done with a
//! set of solid rules and bugs ironed out ... start building tests").
//!
//! The earlier blend_lines split per LINE, which is wrong for multi-line
//! constructs: a fenced code block or a multi-line list should be ONE block,
//! and editing anywhere inside it should keep the WHOLE block raw (so you can
//! edit fence content without it flickering line-by-line). This segments the
//! document into BLOCKS, then marks the block containing the cursor as Raw
//! (editable) and the rest as Rendered. PURE + testable.

/// A document block for the blend.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// 0-based line range [start, end) in the document
    pub start_line: usize,
    pub end_line: usize,
    /// the block's raw text (lines joined with '\n')
    pub raw: String,
    pub kind: BlockKind,
    /// true if the cursor is inside this block → show RAW (editable)
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading,
    FencedCode,
    ListItem,
    Blank,
}

/// v0.1.691 (editor feat 2): block-aware Enter. Given the current LINE (the
/// line the caret is on) and whether the caret is at the line's end, decide
/// what pressing Enter should insert. Keeps structure clean while writing:
/// - inside a list item ("- x" / "1. x"): continue the list (new marker;
///   ordered markers increment). An EMPTY list item ("- ") ends the list
///   (returns just a newline, and signals the marker should be removed).
/// - otherwise: a plain newline.
/// Returns (text_to_insert, remove_current_line_marker).
#[derive(Debug, PartialEq)]
pub struct EnterAction {
    /// text inserted at the caret (replaces the Enter keystroke)
    pub insert: String,
    /// if true, the current line's list marker should be cleared (empty item
    /// → exit the list)
    pub clear_marker: bool,
}

pub fn enter_action(line: &str) -> EnterAction {
    let trimmed = line.trim_start();
    let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();

    // unordered list
    for m in &["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(m) {
            if rest.trim().is_empty() {
                // empty item → exit list
                return EnterAction { insert: "\n".into(), clear_marker: true };
            }
            return EnterAction { insert: format!("\n{}{}", indent, m), clear_marker: false };
        }
    }
    // ordered list: digits then ". " / ") "
    let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let after = &trimmed[digits.len()..];
        for sep in &[". ", ") "] {
            if let Some(rest) = after.strip_prefix(sep) {
                if rest.trim().is_empty() {
                    return EnterAction { insert: "\n".into(), clear_marker: true };
                }
                let n: usize = digits.parse().unwrap_or(1);
                return EnterAction {
                    insert: format!("\n{}{}{}", indent, n + 1, sep),
                    clear_marker: false,
                };
            }
        }
    }
    // plain
    EnterAction { insert: "\n".into(), clear_marker: false }
}

/// v0.1.685: byte offset of the start of line `line_idx` (0-based) in `text`.
/// The editor maps a local cursor inside the active block back to a GLOBAL
/// offset (= line_offset(start_line) + local) so it can re-segment after an
/// edit. Clamps to text length.
pub fn line_offset(text: &str, line_idx: usize) -> usize {
    let mut off = 0usize;
    for (i, l) in text.split('\n').enumerate() {
        if i == line_idx { return off; }
        off += l.len() + 1; // + '\n'
    }
    text.len()
}

/// v0.1.685: replace the lines [start_line, end_line) of `text` with
/// `new_block` (which may itself be multi-line), returning the new document.
/// The editor calls this when the user finishes editing the active (raw)
/// block, to splice it back. Preserves surrounding lines + line structure.
pub fn splice_block(text: &str, start_line: usize, end_line: usize, new_block: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let start = start_line.min(lines.len());
    let end = end_line.min(lines.len());
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    out.extend_from_slice(&lines[..start]);
    // new_block may be multi-line; split so the join stays consistent
    let new_lines: Vec<&str> = new_block.split('\n').collect();
    out.extend_from_slice(&new_lines);
    if end < lines.len() {
        out.extend_from_slice(&lines[end..]);
    }
    out.join("\n")
}

/// Segment `text` into blocks and mark the one containing the cursor (a byte
/// offset) as active. Rules:
/// - A fenced code block (``` … ```) is ONE block, even spanning many lines.
/// - Consecutive list-item lines (`- `, `* `, `1. `) group into ONE list block.
/// - A heading (`#`..) is its own one-line block.
/// - Blank lines are their own blocks (separators).
/// - Otherwise, consecutive non-blank lines form a paragraph block.
pub fn segment(text: &str, cursor: usize) -> Vec<Block> {
    let lines: Vec<&str> = text.split('\n').collect();
    // byte offset at the start of each line, to map cursor → line
    let mut line_starts = Vec::with_capacity(lines.len());
    let mut acc = 0usize;
    for l in &lines {
        line_starts.push(acc);
        acc += l.len() + 1; // + '\n'
    }
    let cur = cursor.min(text.len());
    let cursor_line = line_starts.iter().rposition(|&s| s <= cur).unwrap_or(0);

    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        // fenced code block: one block from open fence to close fence
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let start = i;
            i += 1;
            while i < lines.len()
                && !(lines[i].trim_start().starts_with("```")
                     || lines[i].trim_start().starts_with("~~~")) {
                i += 1;
            }
            if i < lines.len() { i += 1; } // include the closing fence
            blocks.push(make_block(&lines, start, i, BlockKind::FencedCode));
            continue;
        }

        if trimmed.is_empty() {
            blocks.push(make_block(&lines, i, i + 1, BlockKind::Blank));
            i += 1;
            continue;
        }

        if is_heading(trimmed) {
            blocks.push(make_block(&lines, i, i + 1, BlockKind::Heading));
            i += 1;
            continue;
        }

        if is_list_item(trimmed) {
            let start = i;
            while i < lines.len() && is_list_item(lines[i].trim_start()) {
                i += 1;
            }
            blocks.push(make_block(&lines, start, i, BlockKind::ListItem));
            continue;
        }

        // paragraph: consecutive non-blank, non-special lines
        let start = i;
        while i < lines.len() {
            let t = lines[i].trim_start();
            if t.is_empty() || is_heading(t) || is_list_item(t)
                || t.starts_with("```") || t.starts_with("~~~") { break; }
            i += 1;
        }
        blocks.push(make_block(&lines, start, i, BlockKind::Paragraph));
    }

    // mark the block containing the cursor as active
    for b in &mut blocks {
        if cursor_line >= b.start_line && cursor_line < b.end_line {
            b.active = true;
        }
    }
    blocks
}

fn make_block(lines: &[&str], start: usize, end: usize, kind: BlockKind) -> Block {
    let raw = lines[start..end].join("\n");
    Block { start_line: start, end_line: end, raw, kind, active: false }
}

fn is_heading(trimmed: &str) -> bool {
    let h = trimmed.chars().take_while(|&c| c == '#').count();
    h >= 1 && h <= 6 && trimmed[h..].starts_with(' ')
}

fn is_list_item(trimmed: &str) -> bool {
    trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ")
        || {
            // ordered: digits then ". " or ") "
            let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
            digits > 0 && {
                let after = &trimmed[digits..];
                after.starts_with(". ") || after.starts_with(") ")
            }
        }
}

/// Wire for QML: one block per line, "start\tend\tkind\tactive\traw"
/// (raw newlines escaped to \n; the host renders inactive blocks via
/// render_note_block and shows the active one raw).
pub fn to_wire(blocks: &[Block]) -> String {
    blocks.iter().map(|b| {
        let kind = match b.kind {
            BlockKind::Paragraph => "p", BlockKind::Heading => "h",
            BlockKind::FencedCode => "code", BlockKind::ListItem => "li",
            BlockKind::Blank => "blank",
        };
        format!("{}\t{}\t{}\t{}\t{}", b.start_line, b.end_line, kind,
            b.active, b.raw.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', " "))
    }).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_continues_unordered_list() {
        let a = enter_action("- first item");
        assert_eq!(a.insert, "\n- ");
        assert!(!a.clear_marker);
        // indented
        let b = enter_action("  - nested");
        assert_eq!(b.insert, "\n  - ");
    }

    #[test]
    fn enter_increments_ordered_list() {
        let a = enter_action("3. third");
        assert_eq!(a.insert, "\n4. ");
        let b = enter_action("1) one");
        assert_eq!(b.insert, "\n2) ");
    }

    #[test]
    fn enter_on_empty_item_exits_list() {
        let a = enter_action("- ");
        assert_eq!(a.insert, "\n");
        assert!(a.clear_marker);
        let b = enter_action("5. ");
        assert!(b.clear_marker);
    }

    #[test]
    fn enter_plain_paragraph() {
        let a = enter_action("just a sentence");
        assert_eq!(a.insert, "\n");
        assert!(!a.clear_marker);
    }

    #[test]
    fn line_offset_maps_lines() {
        let t = "abc\nde\nf";
        assert_eq!(line_offset(t, 0), 0);
        assert_eq!(line_offset(t, 1), 4);  // after "abc\n"
        assert_eq!(line_offset(t, 2), 7);  // after "abc\nde\n"
    }

    #[test]
    fn splice_replaces_block_lines() {
        let t = "one\ntwo\nthree\nfour";
        // replace lines [1,3) ("two","three") with "X"
        assert_eq!(splice_block(t, 1, 3, "X"), "one\nX\nfour");
        // multi-line replacement
        assert_eq!(splice_block(t, 1, 2, "A\nB"), "one\nA\nB\nthree\nfour");
    }

    #[test]
    fn splice_at_start_and_end() {
        let t = "a\nb\nc";
        assert_eq!(splice_block(t, 0, 1, "Z"), "Z\nb\nc");
        assert_eq!(splice_block(t, 2, 3, "Z"), "a\nb\nZ");
    }

    #[test]
    fn splice_round_trips_with_segment() {
        // segment a doc, edit the active block, splice it back → coherent doc
        let t = "# Title\n\nbody one\nbody two\n\n- item";
        let blocks = segment(t, line_offset(t, 2)); // cursor in the paragraph
        let active = blocks.iter().find(|b| b.active).unwrap();
        let edited = splice_block(t, active.start_line, active.end_line, "EDITED para");
        assert!(edited.contains("EDITED para"));
        assert!(edited.contains("# Title") && edited.contains("- item"));
    }

    fn active_block(text: &str, cursor: usize) -> Block {
        segment(text, cursor).into_iter().find(|b| b.active).unwrap()
    }

    #[test]
    fn paragraph_block_is_active_on_cursor() {
        let t = "first para\n\nsecond para";
        let b = active_block(t, 0);
        assert_eq!(b.kind, BlockKind::Paragraph);
        assert_eq!(b.raw, "first para");
        // cursor in second para
        let b2 = active_block(t, 14);
        assert_eq!(b2.raw, "second para");
    }

    #[test]
    fn fenced_code_is_one_block_even_when_editing_inside() {
        let t = "intro\n\n```rust\nlet x = 1;\nlet y = 2;\n```\n\nafter";
        // cursor inside the fence (on "let y")
        let cursor = t.find("let y").unwrap();
        let b = active_block(t, cursor);
        assert_eq!(b.kind, BlockKind::FencedCode);
        assert!(b.raw.contains("let x = 1;") && b.raw.contains("```rust"),
            "whole fence stays one raw block: {:?}", b.raw);
    }

    #[test]
    fn list_items_group_into_one_block() {
        let t = "- one\n- two\n- three\n\npara";
        let b = active_block(t, 0);
        assert_eq!(b.kind, BlockKind::ListItem);
        assert_eq!(b.raw, "- one\n- two\n- three");
    }

    #[test]
    fn heading_is_its_own_block() {
        let t = "# Title\n\nbody text";
        let b = active_block(t, 2);
        assert_eq!(b.kind, BlockKind::Heading);
        assert_eq!(b.raw, "# Title");
    }

    #[test]
    fn only_one_block_active() {
        let t = "a\n\nb\n\nc";
        let blocks = segment(t, 4); // in "b"
        assert_eq!(blocks.iter().filter(|b| b.active).count(), 1);
    }

    #[test]
    fn ordered_list_detected() {
        let t = "1. first\n2. second";
        let b = active_block(t, 0);
        assert_eq!(b.kind, BlockKind::ListItem);
        assert_eq!(b.raw, "1. first\n2. second");
    }

    #[test]
    fn cursor_at_end_maps_to_last_block() {
        let t = "a\n\nlast";
        let b = active_block(t, t.len());
        assert_eq!(b.raw, "last");
    }

    #[test]
    fn unclosed_fence_does_not_panic_and_extends_to_eof() {
        let t = "```\nno close\nmore";
        let b = active_block(t, t.len());
        assert_eq!(b.kind, BlockKind::FencedCode);
    }

    #[test]
    fn wire_escapes_newlines() {
        let t = "```\nx\n```";
        let w = to_wire(&segment(t, 0));
        assert!(w.contains("\\n"), "raw newlines escaped in wire");
        assert!(!w.contains('\n') || w.matches('\n').count() == segment(t,0).len()-1);
    }
}
