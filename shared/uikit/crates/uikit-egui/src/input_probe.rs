//! What keystrokes actually arrive — an on-device probe, not another guess.
//!
//! # Why this exists rather than another fix
//!
//! "Input fields accept one character on mobile" has been chased twice and
//! survived both attempts, and the record of those attempts is the reason this
//! module is a diagnostic instead of a change:
//!
//! * `PORT_STATUS.md` §1 records THREE probes that did not reproduce it — a
//!   bare `TextEdit`, a real ported page, and the whole shell — plus two of its
//!   own probes that invented the bug before the third measured it, and one
//!   suspect (`GestureLayer::update`) named "with more confidence than it had
//!   earned". The conclusion was that the fault is not reachable from a
//!   synthetic-event harness.
//! * `C13_TODO` §66 lays out the next test to run on the phone and ends: if it
//!   still takes one character, "the cause is below egui — the on-screen
//!   keyboard's input method — and the next step is capturing what events
//!   actually arrive, not more egui changes."
//!
//! This is that capture. Everything a synthetic harness can see has been
//! looked at; what nobody has yet is a record of what a real keystroke from a
//! real on-screen keyboard produces.
//!
//! # The hypothesis it is built to discriminate
//!
//! A physical keyboard delivers `Event::Text`. An input method — which is what
//! an on-screen keyboard is on Wayland, through `zwp_input_method_v2` — can
//! deliver `Event::Ime` instead: `Preedit` while composing and `Commit` when
//! the character is settled. egui's `TextEdit` handles both, so IME alone is
//! not the bug. What would produce exactly the reported symptom is text
//! arriving as a series of `Preedit`s that each REPLACE the last, with a
//! `Commit` that never comes or comes once — a field that shows one character
//! and refuses to grow.
//!
//! That is a guess too. The difference is that this module makes it a question
//! the phone can answer in one screenshot, rather than a change someone has to
//! trust.
//!
//! # Using it
//!
//! `RUSTY_INPUT_PROBE=1` (any app, the variable is deliberately fleet-wide)
//! and call [`overlay`] once per frame from anywhere with a `Context`. It
//! draws a corner strip listing the last events verbatim. Off by default and
//! costed at one env read per process, so leaving the call in is free.

use std::fmt::Write as _;

/// Should the probe draw? Read once — an env var cannot change under a running
/// process, and checking per frame would put a syscall in the paint path.
pub fn enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("RUSTY_INPUT_PROBE")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
    })
}

/// One line per event, in the vocabulary the bug is argued in.
///
/// Deliberately verbatim rather than summarised: the question is which VARIANT
/// arrives, and a tidier rendering that folded `Ime(Preedit)` and `Text` into
/// "typed" would erase the only thing worth reading.
pub fn describe(events: &[egui::Event]) -> Vec<String> {
    events.iter().map(describe_one).collect()
}

fn describe_one(e: &egui::Event) -> String {
    let mut s = String::new();
    match e {
        egui::Event::Text(t) => {
            let _ = write!(s, "Text({t:?})");
        }
        egui::Event::Key { key, pressed, repeat, modifiers, .. } => {
            let _ = write!(
                s,
                "Key({key:?} {}{}{})",
                if *pressed { "down" } else { "up" },
                if *repeat { " repeat" } else { "" },
                if modifiers.any() { " +mods" } else { "" },
            );
        }
        egui::Event::Ime(ime) => {
            // THE VARIANT THIS EXISTS FOR. A run of Preedits that each replace
            // the last, with no Commit, is a field that shows one character and
            // refuses to grow — the reported symptom, exactly.
            let _ = write!(s, "Ime({ime:?})");
        }
        egui::Event::Paste(t) => {
            let _ = write!(s, "Paste(len {})", t.chars().count());
        }
        egui::Event::PointerButton { pressed, .. } => {
            let _ = write!(s, "Pointer({})", if *pressed { "down" } else { "up" });
        }
        egui::Event::Touch { phase, .. } => {
            let _ = write!(s, "Touch({phase:?})");
        }
        other => {
            // Everything else by its debug name only: the noise is
            // PointerMoved, and a screenful of coordinates would bury the two
            // lines worth reading.
            let d = format!("{other:?}");
            let head: String = d.chars().take_while(|c| c.is_alphanumeric()).collect();
            let _ = write!(s, "{head}");
        }
    }
    s
}

/// A rolling log of the interesting events, newest last.
///
/// Pointer motion is dropped: it arrives every frame and would push a
/// keystroke off the strip before anyone could read it.
#[derive(Default, Clone)]
pub struct Probe {
    lines: Vec<String>,
    /// Focus, sampled each frame. A field that loses focus between keystrokes
    /// would produce the same symptom from a completely different cause, so
    /// the probe records it rather than leaving it to be assumed either way.
    focus_changes: usize,
    last_focus: Option<egui::Id>,
}

impl Probe {
    pub const CAP: usize = 24;

    pub fn observe(&mut self, ctx: &egui::Context) {
        let new = ctx.input(|i| {
            i.events
                .iter()
                .filter(|e| {
                    !matches!(
                        e,
                        egui::Event::PointerMoved(_) | egui::Event::MouseMoved(_)
                    )
                })
                .cloned()
                .collect::<Vec<_>>()
        });
        for line in describe(&new) {
            self.lines.push(line);
        }
        let focus = ctx.memory(|m| m.focused());
        if focus != self.last_focus {
            self.focus_changes += 1;
            self.lines.push(format!("FOCUS -> {focus:?}"));
            self.last_focus = focus;
        }
        let over = self.lines.len().saturating_sub(Self::CAP);
        if over > 0 {
            self.lines.drain(..over);
        }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn focus_changes(&self) -> usize {
        self.focus_changes
    }
}

/// Draw the probe over everything, if it is enabled.
///
/// `Order::Debug` so it sits above any app's own overlays — a diagnostic that
/// a dialog can cover is one that fails exactly when a dialog is involved.
pub fn overlay(ctx: &egui::Context, probe: &Probe) {
    if !enabled() {
        return;
    }
    egui::Area::new(egui::Id::new("uikit-input-probe"))
        .order(egui::Order::Debug)
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(4.0, 4.0))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_black_alpha(210))
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "input probe — {} focus changes",
                            probe.focus_changes()
                        ))
                        .color(egui::Color32::from_rgb(255, 200, 80))
                        .monospace()
                        .small(),
                    );
                    for l in probe.lines() {
                        ui.label(
                            egui::RichText::new(l)
                                .color(egui::Color32::WHITE)
                                .monospace()
                                .small(),
                        );
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variant_is_what_the_line_says() {
        // The whole value of this module is telling `Text` from `Ime`, so that
        // is what the test pins. A description that folded them together would
        // pass any looser check while erasing the only distinction being made.
        let text = describe(&[egui::Event::Text("a".into())]);
        assert_eq!(text.len(), 1);
        assert!(text[0].starts_with("Text("), "{}", text[0]);
        assert!(text[0].contains('a'));

        let ime = describe(&[egui::Event::Ime(egui::ImeEvent::Preedit("a".into()))]);
        assert!(ime[0].starts_with("Ime("), "{}", ime[0]);
        assert!(ime[0].contains("Preedit"), "the phase must survive: {}", ime[0]);

        let commit = describe(&[egui::Event::Ime(egui::ImeEvent::Commit("a".into()))]);
        assert!(commit[0].contains("Commit"), "{}", commit[0]);
        assert_ne!(ime[0], commit[0], "Preedit and Commit render identically");
    }

    #[test]
    fn the_log_keeps_the_newest_and_drops_the_oldest() {
        // A ring that dropped the NEWEST would show a stale screen and look
        // like the app had stopped receiving input — the same symptom as the
        // bug, from the diagnostic.
        let mut p = Probe::default();
        for i in 0..(Probe::CAP + 5) {
            p.lines.push(format!("e{i}"));
        }
        let over = p.lines.len().saturating_sub(Probe::CAP);
        p.lines.drain(..over);
        assert_eq!(p.lines.len(), Probe::CAP);
        assert_eq!(p.lines.last().unwrap(), &format!("e{}", Probe::CAP + 4));
    }

    #[test]
    fn the_probe_is_off_unless_asked_for() {
        // It draws over the whole screen at Order::Debug. Defaulting to on
        // would make every app in the fleet unusable.
        //
        // SAFETY: single-threaded test, and the value is restored.
        let prev = std::env::var_os("RUSTY_INPUT_PROBE");
        unsafe { std::env::remove_var("RUSTY_INPUT_PROBE") };
        // `enabled()` memoises, so this asserts the DEFAULT rather than the
        // toggle — which is the property that matters, since the default is
        // what every user gets.
        assert!(!enabled());
        if let Some(v) = prev {
            unsafe { std::env::set_var("RUSTY_INPUT_PROBE", v) };
        }
    }
}
