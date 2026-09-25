//! input_log -- what arrives from the platform, frame by frame, for
//! `SNAPCLONE_INPUT_LOG=1`.
//!
//! Written for a report no test here reproduces: on Fedora Asahi, text fields
//! (the conversation draft, "search friends", add a peer, add a landmark) keep
//! one character. Driven here with Tab or a click, plain Text, real Key events,
//! and IME Preedit/Commit, with the binary's per-frame settings applied, every
//! one of those fields keeps every character. So the difference is in what the
//! platform actually delivers, and this makes it visible: keys, text, IME,
//! clicks and window focus, plus which widget holds focus when that changes.

/// One line for a frame, or None when nothing relevant happened. `focus` is
/// the focused widget after the frame and `was` the one before it.
pub fn line(events: &[egui::Event], was: Option<egui::Id>, focus: Option<egui::Id>) -> Option<String> {
    use egui::Event;
    let mut parts: Vec<String> = Vec::new();
    for e in events {
        match e {
            Event::Key { key, pressed, repeat, modifiers, .. } => {
                parts.push(format!("key {key:?} {}{}{}", if *pressed { "down" } else { "up" },
                    if *repeat { " repeat" } else { "" },
                    if modifiers.any() { format!(" {modifiers:?}") } else { String::new() }));
            }
            Event::Text(t) => parts.push(format!("text {t:?}")),
            Event::Ime(i) => parts.push(format!("ime {i:?}")),
            Event::PointerButton { pos, pressed, .. } => {
                parts.push(format!("click {} at ({:.0},{:.0})", if *pressed { "down" } else { "up" }, pos.x, pos.y));
            }
            Event::WindowFocused(f) => parts.push(format!("window focused {f}")),
            _ => {}
        }
    }
    if was != focus {
        parts.push(format!("focus {was:?} -> {focus:?}"));
    }
    if parts.is_empty() { None } else { Some(parts.join(" | ")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, ImeEvent, Key, Modifiers};

    #[test]
    fn a_frame_is_reduced_to_the_input_that_matters() {
        let id = egui::Id::new("draft");
        let evs = vec![
            Event::PointerMoved(egui::pos2(1.0, 2.0)),
            Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE },
            Event::Text("a".into()),
            Event::Ime(ImeEvent::Commit("b".into())),
            Event::WindowFocused(false),
        ];
        let l = line(&evs, Some(id), None).unwrap();
        assert!(l.contains("key A down") && l.contains("text \"a\"") && l.contains("Commit"), "{l}");
        assert!(l.contains("window focused false") && l.contains("focus Some"), "{l}");
        assert!(!l.contains("PointerMoved") && !l.contains("(1,2)"), "pointer motion is left out: {l}");
        assert_eq!(line(&[Event::PointerMoved(egui::pos2(0.0, 0.0))], Some(id), Some(id)), None, "a quiet frame logs nothing");
    }
}
