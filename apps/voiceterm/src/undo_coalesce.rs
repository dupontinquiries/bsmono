//! v0.1.692: undo "smart separation" — the boundary logic that decides when
//! a new edit should START a new undo entry vs extend the current one, so
//! undo lands on meaningful units (a word, a line, a paste, a pause-delimited
//! burst) instead of one keystroke at a time. See docs/design/editor_undo.md.
//!
//! PURE + testable. This is the foundation; the snapshot ring, qt_methods,
//! voice intents, and visual timeline are designed in the doc and await
//! sign-off. Nothing here is wired live yet.

/// The kind of an edit, derived by diffing the previous vs next doc length +
/// the just-typed char. Coarse on purpose — we only need it to detect KIND
/// CHANGES (typing→deleting, word→whitespace) for boundary decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// inserted a word character (letter/digit/_)
    InsertWord,
    /// inserted whitespace or a newline (structure)
    InsertSpace,
    /// inserted a newline specifically (always a boundary)
    InsertNewline,
    /// inserted punctuation / symbol
    InsertPunct,
    /// removed text (backspace/delete)
    Delete,
    /// a large change (paste / cut / programmatic splice)
    Bulk,
}

/// Classify an edit from old/new lengths and the char at the caret (the last
/// inserted char, if an insertion). `inserted` None ⇒ a deletion.
pub fn edit_kind(old_len: usize, new_len: usize, inserted: Option<char>) -> EditKind {
    let delta = new_len as isize - old_len as isize;
    if delta.unsigned_abs() > 80 {
        return EditKind::Bulk;
    }
    match inserted {
        None => EditKind::Delete,
        Some(c) => {
            if c == '\n' { EditKind::InsertNewline }
            else if c.is_whitespace() { EditKind::InsertSpace }
            else if c.is_alphanumeric() || c == '_' { EditKind::InsertWord }
            else { EditKind::InsertPunct }
        }
    }
}

/// Decide whether to COMMIT A BOUNDARY before applying `next` (i.e. close the
/// current undo entry and start a new one). `dt_ms` is the time since the
/// last edit. `prev` is the kind of the previous edit in the current entry
/// (None if the entry is empty/fresh).
///
/// Rules (any → boundary):
/// - time pause > pause_ms (a thought unit ended)
/// - newline / bulk are always their own boundary
/// - kind change between the word/space/delete families
/// - a word boundary: word chars followed by a space/punct closes the word
pub fn should_break(prev: Option<EditKind>, next: EditKind, dt_ms: u64,
                    pause_ms: u64) -> bool {
    let prev = match prev {
        None => return false, // nothing to break from yet
        Some(p) => p,
    };
    if dt_ms > pause_ms { return true; }
    // structural edits stand alone
    if matches!(next, EditKind::InsertNewline | EditKind::Bulk) { return true; }
    if matches!(prev, EditKind::InsertNewline | EditKind::Bulk) { return true; }
    // typing vs deleting are different acts
    let prev_del = prev == EditKind::Delete;
    let next_del = next == EditKind::Delete;
    if prev_del != next_del { return true; }
    // word closes when followed by whitespace/punct (so undo eats a word)
    if prev == EditKind::InsertWord
        && matches!(next, EditKind::InsertSpace | EditKind::InsertPunct) {
        return true;
    }
    // starting a new word after a space → new unit
    if matches!(prev, EditKind::InsertSpace | EditKind::InsertPunct)
        && next == EditKind::InsertWord {
        return true;
    }
    false
}

/// A short human/voice label for an entry's dominant kind — used by the
/// visual timeline ticks and "undo the last <thing>" voice matching.
pub fn kind_label(k: EditKind) -> &'static str {
    match k {
        EditKind::InsertWord => "word",
        EditKind::InsertSpace => "space",
        EditKind::InsertNewline => "line",
        EditKind::InsertPunct => "punctuation",
        EditKind::Delete => "delete",
        EditKind::Bulk => "paste",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_basic_kinds() {
        assert_eq!(edit_kind(3, 4, Some('a')), EditKind::InsertWord);
        assert_eq!(edit_kind(3, 4, Some(' ')), EditKind::InsertSpace);
        assert_eq!(edit_kind(3, 4, Some('\n')), EditKind::InsertNewline);
        assert_eq!(edit_kind(3, 4, Some('!')), EditKind::InsertPunct);
        assert_eq!(edit_kind(4, 3, None), EditKind::Delete);
        assert_eq!(edit_kind(10, 200, Some('x')), EditKind::Bulk);
        assert_eq!(edit_kind(200, 10, None), EditKind::Bulk);
    }

    #[test]
    fn pause_forces_boundary() {
        // same kind, but a long pause → break
        assert!(should_break(Some(EditKind::InsertWord), EditKind::InsertWord, 900, 700));
        // quick continuation of the same word → no break
        assert!(!should_break(Some(EditKind::InsertWord), EditKind::InsertWord, 80, 700));
    }

    #[test]
    fn newline_and_bulk_stand_alone() {
        assert!(should_break(Some(EditKind::InsertWord), EditKind::InsertNewline, 10, 700));
        assert!(should_break(Some(EditKind::InsertNewline), EditKind::InsertWord, 10, 700));
        assert!(should_break(Some(EditKind::InsertWord), EditKind::Bulk, 10, 700));
    }

    #[test]
    fn typing_vs_deleting_breaks() {
        assert!(should_break(Some(EditKind::InsertWord), EditKind::Delete, 10, 700));
        assert!(should_break(Some(EditKind::Delete), EditKind::InsertWord, 10, 700));
        // continuing to delete → no break
        assert!(!should_break(Some(EditKind::Delete), EditKind::Delete, 10, 700));
    }

    #[test]
    fn word_boundaries() {
        // word then space closes the word
        assert!(should_break(Some(EditKind::InsertWord), EditKind::InsertSpace, 10, 700));
        // space then word starts a new unit
        assert!(should_break(Some(EditKind::InsertSpace), EditKind::InsertWord, 10, 700));
        // continuing the same word → no break
        assert!(!should_break(Some(EditKind::InsertWord), EditKind::InsertWord, 10, 700));
    }

    #[test]
    fn fresh_entry_never_breaks() {
        assert!(!should_break(None, EditKind::InsertWord, 9999, 700));
    }

    #[test]
    fn labels_exist() {
        assert_eq!(kind_label(EditKind::InsertNewline), "line");
        assert_eq!(kind_label(EditKind::Bulk), "paste");
    }
}
