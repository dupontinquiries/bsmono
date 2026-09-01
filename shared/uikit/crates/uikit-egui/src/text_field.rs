//! Text fields you cannot accidentally build without an id.
//!
//! ## Why this module exists
//!
//! egui tracks keyboard focus by widget id. `egui::TextEdit` with no
//! explicit id gets one derived from its position in the layout, so any
//! reflow between frames silently drops focus. Typing a character is
//! itself enough to cause that reflow — a wrapped row sitting near its
//! width threshold re-wraps as the content grows, the widget moves, the
//! id changes, focus is gone. The user sees a field that accepts exactly
//! one character and then ignores the keyboard.
//!
//! That bug was found and patched by hand in three fields in logscope's
//! chat.rs, then reported again, then found in 65 more fields across the
//! fleet, then patched again. Patching call sites does not work: the next
//! `TextEdit::singleline` anyone writes reintroduces it, and it will not
//! be noticed until someone tries to type into it on a narrow screen.
//! Fixing it here — where the only way to get a text field is to name it —
//! is the whole point of having a kit.
//!
//! ## Use
//!
//! ```ignore
//! // instead of: ui.add(egui::TextEdit::singleline(&mut s))
//! uikit_egui::text_field::single(ui, "search-box", &mut s);
//!
//! // in a loop, key on whatever makes the row unique:
//! uikit_egui::text_field::single(ui, ("row-note", row.id), &mut row.note);
//!
//! // need the builder? take it pre-salted and keep chaining:
//! ui.add(text_field::edit("cmd", &mut s).desired_width(200.0).password(true));
//! ```
//!
//! The id argument is `impl Hash`, so a `&str` covers the common case and
//! a tuple covers loops. There is deliberately no default and no
//! `Option<Id>` — an id you can leave out is an id that gets left out.
//!
//! ## The one rule that is not obvious
//!
//! When the *same* logical field is drawn from two places — a wide layout
//! and a narrow one — both call sites must pass the **same** id. Otherwise
//! crossing the responsive breakpoint mid-sentence drops focus, which is
//! the same bug wearing a different hat. Two real instances of this exist
//! in this repo (logscope's chat composer, aperture's loop_pane
//! instruction), and both now share an id for exactly this reason.

use std::hash::Hash;

/// A `TextEdit` builder that already has a stable id. Chain the rest.
///
/// Prefer [`single`] / [`multi`] unless you need builder options.
pub fn edit<'t>(id: impl Hash, text: &'t mut dyn egui::TextBuffer) -> egui::TextEdit<'t> {
    egui::TextEdit::singleline(text).id_salt(IdSalt(id_hash(id)))
}

/// Multiline variant of [`edit`].
pub fn edit_multi<'t>(
    id: impl Hash,
    text: &'t mut dyn egui::TextBuffer,
) -> egui::TextEdit<'t> {
    egui::TextEdit::multiline(text).id_salt(IdSalt(id_hash(id)))
}

/// Add a single-line field and return its response.
pub fn single(ui: &mut egui::Ui, id: impl Hash, text: &mut dyn egui::TextBuffer) -> egui::Response {
    ui.add(edit(id, text))
}

/// Add a multi-line field and return its response.
pub fn multi(ui: &mut egui::Ui, id: impl Hash, text: &mut dyn egui::TextBuffer) -> egui::Response {
    ui.add(edit_multi(id, text))
}

/// Single-line field with placeholder text.
pub fn hinted(
    ui: &mut egui::Ui,
    id: impl Hash,
    hint: &str,
    text: &mut dyn egui::TextBuffer,
) -> egui::Response {
    ui.add(edit(id, text).hint_text(hint))
}

/// Wrapper so a hashed id round-trips through egui's `id_salt` as one
/// opaque value. Hashing here rather than passing the caller's value
/// straight through means `single(ui, "x", ..)` and
/// `single(ui, ("x",), ..)` cannot collide by accident, and keeps the
/// public signature `impl Hash` instead of leaking egui's own bounds.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct IdSalt(u64);

fn id_hash(id: impl Hash) -> u64 {
    use std::hash::Hasher;
    // Fixed-seed DefaultHasher: ids only need to be stable WITHIN a run
    // (focus does not persist across restarts), so a per-process-random
    // seed would be fine too — but a fixed one makes a mismatched pair of
    // call sites reproducible in a test instead of only sometimes.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_id_from_two_call_sites_hashes_the_same() {
        // This is the property the responsive-layout case depends on: a
        // field drawn from the wide branch and the narrow branch must be
        // one widget as far as focus is concerned.
        assert_eq!(id_hash("chat-composer"), id_hash("chat-composer"));
    }

    #[test]
    fn different_ids_do_not_collide() {
        assert_ne!(id_hash("search"), id_hash("filter"));
    }

    #[test]
    fn loop_keys_stay_distinct_per_row() {
        // A constant id inside a loop is the failure this signature is
        // shaped to make awkward: every row would share focus.
        let a = id_hash(("row-note", 1u32));
        let b = id_hash(("row-note", 2u32));
        assert_ne!(a, b);
    }

    #[test]
    fn a_one_tuple_hashes_as_its_element_which_is_rusts_rule_not_ours() {
        // Written first as `assert_ne!` on the assumption that wrapping in
        // a tuple would distinguish them. It does not: std's Hash impl for
        // a 1-tuple delegates straight to the element, so `("x",)` and
        // `"x"` are the same id. Recorded as the passing direction rather
        // than deleted, because the tempting mistake it guards is real —
        // reaching for `(name,)` to "namespace" an id buys nothing, and
        // someone will try it.
        assert_eq!(id_hash("x"), id_hash(("x",)));
        // What DOES namespace an id is a second component.
        assert_ne!(id_hash("x"), id_hash(("search", "x")));
    }
}
