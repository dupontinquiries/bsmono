//! v0.1.672: note-editor "WYSIWYG blend" core — the logic that makes
//! ZBrackets come alive while editing.
//!
//! The edit mode is a live-preview blend: the line the cursor is on shows raw
//! markdown source (so you can edit `[[name]]` syntax), every other line
//! renders with ZBrackets as live links (zbrackets::rewrite). As you type
//! `[[`, an autocomplete dropdown (note_graph::rank) offers completions;
//! picking one rewrites the open token.
//!
//! PURE + testable: cursor/typing-context detection, completion accept, and
//! the active-line split are here; the TextEdit/ListView widget + dropdown
//! are the QML layer.

/// What the cursor is doing relative to a ZBracket token, for the
/// autocomplete trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditContext {
    /// cursor is inside an unclosed `[[` on the current line
    pub in_link: bool,
    /// text typed after the `[[` up to the cursor (the autocomplete partial)
    pub partial: String,
    /// byte offset of the opening `[[` (for replacing on accept)
    pub link_start: usize,
}

impl EditContext {
    fn none() -> Self { EditContext { in_link: false, partial: String::new(), link_start: 0 } }
}

/// Largest char boundary <= `i` (clamps an arbitrary byte offset coming from
/// the UI to a safe slice point).
fn clamp_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) { i -= 1; }
    i
}

/// Detect whether the cursor (byte offset) sits inside an unclosed `[[` on
/// its line, and if so the partial typed so far. Drives the completion
/// dropdown: fire it when `in_link`, query = `partial`.
pub fn zb_cursor_context(text: &str, cursor: usize) -> EditContext {
    let cur = clamp_boundary(text, cursor);
    let before = &text[..cur];
    // nearest "[[" before the cursor
    let open = match before.rfind("[[") { Some(o) => o, None => return EditContext::none() };
    let between = &before[open + 2..];
    // must be unclosed (no "]]") and on the same line (no newline)
    if between.contains("]]") || between.contains('\n') {
        return EditContext::none();
    }
    EditContext { in_link: true, partial: between.to_string(), link_start: open }
}

/// Accept a completion: replace the open `[[partial` (from `ctx.link_start`
/// up to `cursor`) with `[[chosen]]` and return (new_text, new_cursor) with
/// the cursor placed just after the inserted `]]`. If there's already a `]]`
/// immediately after the cursor we don't double it.
pub fn accept_completion(text: &str, cursor: usize, ctx: &EditContext, chosen: &str)
    -> (String, usize) {
    if !ctx.in_link { return (text.to_string(), cursor); }
    let cur = clamp_boundary(text, cursor);
    let start = ctx.link_start.min(text.len());
    // does a closing ]] already follow the cursor (user typed it / live render)?
    let tail = &text[cur..];
    let already_closed = tail.starts_with("]]");
    let insert = if already_closed { format!("[[{}", chosen) }
                 else { format!("[[{}]]", chosen) };
    let mut out = String::with_capacity(text.len() + insert.len());
    out.push_str(&text[..start]);
    out.push_str(&insert);
    out.push_str(tail);
    // cursor after the closing ]]
    let new_cursor = if already_closed { start + insert.len() + 2 }
                     else { start + insert.len() };
    (out, new_cursor)
}

/// 0-based index of the line containing the cursor byte offset. Used to pick
/// the "active" line (shown as raw source) in the live-preview blend.
pub fn cursor_line(text: &str, cursor: usize) -> usize {
    let cur = clamp_boundary(text, cursor);
    text[..cur].bytes().filter(|&b| b == b'\n').count()
}

/// Split text into lines tagged active/inactive for the blend. The active
/// line (cursor's) is shown raw; others render. (The render itself is done
/// by the caller via zbrackets::rewrite, so this stays decoupled + trivially
/// correct.)
pub fn blend_lines(text: &str, cursor: usize) -> Vec<(String, bool)> {
    let active = cursor_line(text, cursor);
    text.split('\n').enumerate()
        .map(|(i, l)| (l.to_string(), i == active))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_open_link_and_partial() {
        let text = "see [[Da";
        let ctx = zb_cursor_context(text, text.len());
        assert!(ctx.in_link);
        assert_eq!(ctx.partial, "Da");
        assert_eq!(ctx.link_start, 4);
    }

    #[test]
    fn closed_link_is_not_open() {
        let text = "see [[Done]] now";
        // cursor after the closed link
        let ctx = zb_cursor_context(text, text.len());
        assert!(!ctx.in_link);
    }

    #[test]
    fn newline_breaks_the_link_context() {
        let text = "[[start\nmore";
        let ctx = zb_cursor_context(text, text.len());
        assert!(!ctx.in_link, "a link can't span a newline");
    }

    #[test]
    fn no_brackets_no_context() {
        assert!(!zb_cursor_context("plain text", 5).in_link);
    }

    #[test]
    fn cursor_inside_just_after_open() {
        let text = "x [[";
        let ctx = zb_cursor_context(text, text.len());
        assert!(ctx.in_link);
        assert_eq!(ctx.partial, "");
    }

    #[test]
    fn accept_inserts_full_link() {
        let text = "see [[Da";
        let ctx = zb_cursor_context(text, text.len());
        let (out, cur) = accept_completion(text, text.len(), &ctx, "Database");
        assert_eq!(out, "see [[Database]]");
        assert_eq!(cur, out.len());
    }

    #[test]
    fn accept_does_not_double_existing_close() {
        // live-render may have already shown the ]] right after the cursor
        let text = "see [[Da]]";
        let cursor = 8; // right before the ]]
        let ctx = zb_cursor_context(text, cursor);
        assert!(ctx.in_link && ctx.partial == "Da");
        let (out, _cur) = accept_completion(text, cursor, &ctx, "Database");
        assert_eq!(out, "see [[Database]]", "no doubled brackets");
    }

    #[test]
    fn accept_preserves_surrounding_text() {
        let text = "before [[par after";
        let cursor = 12; // after "par"
        let ctx = zb_cursor_context(text, cursor);
        let (out, _) = accept_completion(text, cursor, &ctx, "Parser");
        assert_eq!(out, "before [[Parser]] after");
    }

    #[test]
    fn cursor_line_index() {
        let text = "a\nb\nc";
        assert_eq!(cursor_line(text, 0), 0);
        assert_eq!(cursor_line(text, 2), 1);
        assert_eq!(cursor_line(text, 4), 2);
    }

    #[test]
    fn blend_marks_active_line() {
        let text = "one\ntwo\nthree";
        let b = blend_lines(text, 5); // offset in "two"
        assert_eq!(b[0], ("one".into(), false));
        assert_eq!(b[1], ("two".into(), true)); // active → raw
        assert_eq!(b[2], ("three".into(), false));
    }

    #[test]
    fn boundary_clamp_does_not_panic_midchar() {
        let text = "café [[Da"; // 'é' is 2 bytes
        // a cursor offset that may land mid-char is clamped safely
        for c in 0..=text.len() + 2 {
            let _ = zb_cursor_context(text, c); // must not panic
        }
    }
}
