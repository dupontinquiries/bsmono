//! note_blend_view — the blend, rendered.
//!
//! Two halves were here and never met. `zbrackets` has been `#[path]`-mounted
//! in this crate with **no caller at all** — the frontend could rewrite
//! `[[name]]` into a link and never did. `note_blend` (`src/note_edit.rs`) is
//! the half that decides WHICH lines to rewrite, and it was not mounted.
//!
//! Neither is useful alone, which is presumably how both came to sit there:
//! `blend_lines` returns lines tagged active/inactive and deliberately does no
//! rendering (*"the render itself is done by the caller via
//! zbrackets::rewrite, so this stays decoupled + trivially correct"*), and
//! `zbrackets::rewrite` will happily rewrite a line that should have stayed
//! raw. The decoupling is right; it just left a seam nobody had crossed.
//!
//! ## What the blend is for
//!
//! The cursor's line shows raw markdown so `[[name]]` can be edited as text.
//! Every other line renders as a live link. That is the whole trick: you are
//! always editing source, and you are never looking at source you are not
//! editing.
//!
//! Getting it backwards — rendering the active line — is the failure worth
//! naming, because it is subtle and it is what a naive "render everything"
//! does. The `[[` you just typed turns into a link mid-keystroke, the cursor
//! lands somewhere unrelated, and the next character goes into the wrong
//! place. The test for that is [`the_cursor_line_is_never_rewritten`].

/// One line of the blended view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlendLine {
    /// What to draw. Raw source on the active line, rewritten elsewhere.
    pub text: String,
    /// True on the line holding the cursor.
    pub active: bool,
    /// Whether `text` differs from the source — i.e. the renderer did
    /// something. Lets a view style rewritten lines without diffing.
    pub rewritten: bool,
}

/// Render a note for editing at `cursor`.
///
/// `render` is the rewriter — a closure in tests, and in the app:
///
/// ```ignore
/// blend(&note, cursor, |line| zbrackets::rewrite(line, &path, &index))
/// ```
///
/// Passed in rather than called directly because `rewrite`'s real signature is
/// `(markdown, current_path, index)` — it needs the note's own path and a map
/// of every known note title, both of which the shell owns. Calling it from
/// here would drag a note index into a function whose whole job is deciding
/// which lines to skip.
pub fn blend<F>(text: &str, cursor: usize, render: F) -> Vec<BlendLine>
where
    F: Fn(&str) -> String,
{
    crate::note_blend::blend_lines(text, cursor)
        .into_iter()
        .map(|(line, active)| {
            if active {
                // THE RULE. Never rewrite the line being edited.
                BlendLine { text: line, active: true, rewritten: false }
            } else {
                let out = render(&line);
                let rewritten = out != line;
                BlendLine { text: out, active: false, rewritten }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rewriter that makes the change visible without pulling in the real
    /// link resolution.
    fn fake(line: &str) -> String {
        line.replace("[[", "<").replace("]]", ">")
    }

    /// THE RULE, and the subtle failure it prevents: if the active line were
    /// rewritten, the `[[` you just typed would become a link mid-keystroke,
    /// the cursor would land somewhere unrelated, and the next character would
    /// go into the wrong place.
    #[test]
    fn the_cursor_line_is_never_rewritten() {
        let text = "one [[a]]\ntwo [[b]]\nthree [[c]]";
        // Cursor on line 1 (the "two" line).
        let cursor = text.find("two").unwrap() + 1;
        let out = blend(text, cursor, fake);
        assert_eq!(out[1].text, "two [[b]]", "the edited line stays source");
        assert!(out[1].active && !out[1].rewritten);
        assert_eq!(out[0].text, "one <a>", "the others render");
        assert_eq!(out[2].text, "three <c>");
    }

    /// Exactly one line is active, wherever the cursor is.
    #[test]
    fn exactly_one_line_is_active() {
        let text = "a\nb\nc\nd";
        for cursor in 0..=text.len() {
            let out = blend(text, cursor, fake);
            assert_eq!(out.iter().filter(|l| l.active).count(), 1,
                "cursor {cursor} produced {} active lines", out.iter().filter(|l| l.active).count());
        }
    }

    /// `rewritten` reports what the renderer DID, so a view can style changed
    /// lines without diffing them itself. A line with no link is not marked.
    #[test]
    fn rewritten_tracks_whether_the_renderer_changed_anything() {
        let out = blend("plain text\n[[link]]\nmore", 0, fake);
        assert!(!out[1].active);
        assert!(out[1].rewritten, "the link line changed");
        assert!(!out[2].rewritten, "the plain line did not");
    }

    /// Empty and single-line notes are ordinary, not edge cases to crash on.
    #[test]
    fn degenerate_notes_still_blend() {
        assert_eq!(blend("", 0, fake).len(), 1, "an empty note is one empty line");
        let one = blend("[[x]]", 0, fake);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].text, "[[x]]", "the only line holds the cursor, so it stays raw");
    }

    /// A cursor past the end clamps rather than panicking — an editor can ask
    /// after a deletion before it has re-clamped its own state.
    #[test]
    fn a_cursor_past_the_end_lands_on_the_last_line() {
        let out = blend("a\nb", 9999, fake);
        assert!(out.last().unwrap().active);
    }

    /// The renderer is never handed the active line, so a caller cannot make
    /// it expensive for the line being typed into.
    #[test]
    fn the_renderer_is_not_called_for_the_active_line() {
        use std::cell::RefCell;
        let seen = RefCell::new(Vec::new());
        let text = "one\ntwo\nthree";
        let cursor = text.find("two").unwrap();
        blend(text, cursor, |l| {
            seen.borrow_mut().push(l.to_string());
            l.to_string()
        });
        assert!(!seen.borrow().contains(&"two".to_string()),
            "rendered {:?}", seen.borrow());
        assert_eq!(seen.borrow().len(), 2);
    }
}
