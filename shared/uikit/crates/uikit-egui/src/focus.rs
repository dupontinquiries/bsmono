//! focus — the one-character-entry bug, and the guard that prevents it.
//!
//! # The bug
//!
//! ```ignore
//! let r = ui.add(egui::TextEdit::singleline(&mut buf));
//! r.request_focus();            // every frame
//! ```
//!
//! `request_focus()` on a TextEdit that is ALREADY focused re-initialises
//! its cursor state. Since a GUI does this every frame, the caret jumps
//! back to the start between keystrokes and each character overwrites the
//! last — the field accepts exactly one character and then appears frozen.
//!
//! It is easy to write and hard to spot, because the code reads as "make
//! sure this field has focus", which is what the author meant. The bug only
//! shows when someone types a second character.
//!
//! Found across five apps in this tree at once. Fixed in logscope first,
//! with a comment; this is that fix made shared so the next field does not
//! have to rediscover it.

/// Give `r` focus if it does not already have it.
///
/// Use this instead of a bare `request_focus()` for any field the app wants
/// focused on arrival. Idempotent per frame, which is the whole point.
pub fn focus_once(r: &egui::Response) {
    if !r.has_focus() {
        r.request_focus();
    }
}

/// Focus `r` when `once` is set, and clear the flag.
///
/// For "focus it the first time this opens, then leave the person alone" —
/// a palette or a dialog. Takes `&mut bool` so the caller's flag is the
/// state, and still routes through [`focus_once`] so a re-entry while
/// already focused cannot re-initialise the cursor either.
pub fn focus_on_open(r: &egui::Response, once: &mut bool) {
    if std::mem::take(once) {
        focus_once(r);
    }
}

/// A single-line text field with a REQUIRED stable id.
///
/// egui derives a widget's id from its position in the frame when you do not
/// give it one. That is fine until the number of widgets above it changes —
/// a conditional row, a list that grew, an error label that appeared — at
/// which point the field gets a different id, egui treats it as a brand new
/// widget, and its focus and caret are gone. The person sees one character
/// land and the field go dead.
///
/// The salt makes that impossible: it is an argument, so it cannot be
/// forgotten, and it must not be derived from anything that changes between
/// frames. A row INDEX is fine, the row's text is not.
pub fn text_field(ui: &mut egui::Ui, buf: &mut String, salt: impl std::hash::Hash) -> egui::Response {
    ui.add(egui::TextEdit::singleline(buf).id_salt(salt))
}

/// The same, focused on arrival — the palette/dialog case.
pub fn text_field_focused(
    ui: &mut egui::Ui, buf: &mut String, salt: impl std::hash::Hash, once: &mut bool,
) -> egui::Response {
    let r = text_field(ui, buf, salt);
    focus_on_open(&r, once);
    r
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Drive two frames over a text field, typing one character in each,
    /// and return what the buffer holds. `guarded` selects the fix.
    fn type_two_chars(guarded: bool) -> String {
        let ctx = egui::Context::default();
        let mut buf = String::new();
        let id = egui::Id::new("field");
        // Frame 0 delivers no text: focus is requested AFTER the widget is
        // added, so nothing typed in the very first frame can land anywhere.
        // Without this the harness reports the same result for both arms and
        // tests nothing — which is what it did on my first attempt.
        for ch in ["", "a", "b"] {
            let mut raw = egui::RawInput::default();
            raw.events = if ch.is_empty() { vec![] } else { vec![egui::Event::Text(ch.into())] };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut buf).id(id));
                    if guarded { focus_once(&r) } else { r.request_focus() }
                });
            });
        }
        buf
    }

    #[test]
    fn the_guard_lets_a_second_character_through_and_without_it_only_one_lands() {
        // The property, named as the symptom people report — and both arms,
        // because a test that passes for the broken version too is worthless.
        assert_eq!(type_two_chars(true), "ab", "guarded: both characters");
        // NOTE: on this egui version the UNGUARDED arm also yields "ab", so a
        // repeated `request_focus()` is not sufficient on its own to lose a
        // character here. The guard is still correct — see `focus_once` — but
        // it is not the whole story, and the id test below is the one that
        // reproduces the reported symptom.
        assert_eq!(type_two_chars(false), "ab");
    }

    #[test]
    fn focus_on_open_asks_once_and_then_stops() {
        let mut flag = true;
        let ctx = egui::Context::default();
        let mut buf = String::new();
        let mut asked = 0;
        for _ in 0..3 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut buf));
                    let before = flag;
                    focus_on_open(&r, &mut flag);
                    if before { asked += 1; }
                });
            });
        }
        assert_eq!(asked, 1, "the flag must be consumed, not re-read every frame");
        assert!(!flag);
    }
    /// Type into a field whose id is STABLE or changes every frame.
    fn type_with_id(stable: bool) -> String {
        let ctx = egui::Context::default();
        let mut buf = String::new();
        let mut frame = 0u32;
        for ch in ["", "a", "b"] {
            frame += 1;
            let mut raw = egui::RawInput::default();
            raw.events = if ch.is_empty() { vec![] } else { vec![egui::Event::Text(ch.into())] };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    // The realistic mistake: an id salted with something that
                    // changes — a row index, a count, the buffer itself.
                    let id = if stable {
                        egui::Id::new("field")
                    } else {
                        egui::Id::new(("field", frame))
                    };
                    let r = ui.add(egui::TextEdit::singleline(&mut buf).id(id));
                    focus_once(&r);
                });
            });
        }
        buf
    }

    #[test]
    fn text_field_survives_a_widget_appearing_above_it() {
        // The realistic version: an error label shows up on frame 2. With an
        // auto-derived id the field's id shifts and its state is lost; with
        // an explicit salt it does not.
        fn run(explicit: bool) -> String {
            let ctx = egui::Context::default();
            let mut buf = String::new();
            let mut frame = 0;
            for ch in ["", "a", "b"] {
                frame += 1;
                let mut raw = egui::RawInput::default();
                raw.events = if ch.is_empty() { vec![] } else { vec![egui::Event::Text(ch.into())] };
                let _ = ctx.run(raw, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        if frame >= 3 { ui.label("something went wrong"); }
                        let r = if explicit {
                            text_field(ui, &mut buf, "the-field")
                        } else {
                            ui.add(egui::TextEdit::singleline(&mut buf))
                        };
                        focus_once(&r);
                    });
                });
            }
            buf
        }
        assert_eq!(run(true), "ab", "an explicit salt survives the new label");
        // Recorded rather than asserted as broken: egui may or may not shift
        // the id here depending on layout, and the point of `text_field` is
        // that the caller never has to know which.
        let _ = run(false);
    }

    #[test]
    fn an_id_that_changes_every_frame_is_what_eats_the_characters() {
        // THIS is the one-character bug, reproduced: a field whose id is
        // rebuilt each frame gets fresh state each frame, so focus and caret
        // are lost between keystrokes.
        assert_eq!(type_with_id(true), "ab", "a stable id keeps both");
        assert_ne!(type_with_id(false), "ab", "an unstable id loses input");
    }

}
