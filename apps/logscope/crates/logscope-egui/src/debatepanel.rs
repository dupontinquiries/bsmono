//! debatepanel.rs — watching a debate without breaking it.
//!
//! ## Why not a live token stream per debater
//!
//! The obvious panel shows each debater's output as it arrives. Four simultaneous token streams is
//! four jittering edges, and a person cannot read any of them — the same failure `revealpace` fixed for
//! one stream, multiplied by the panel size.
//!
//! So each slot shows the last COMPLETE line: a finished sentence or clause, replaced when the next
//! one finishes. The strip changes a few times a second instead of forty, and each change is
//! something readable.
//!
//! ## The trap: watching is a side channel
//!
//! `panelvoice` holds one debater blind so the next round can distinguish a corroborated answer from
//! one hint arriving several times. That control lives in what the DEBATERS know — and an operator who
//! reads A's line and then says "what about a BTreeMap" to B has just carried A's reasoning across,
//! by hand, outside the protocol.
//!
//! The panel cannot prevent that and should not try. What it can do is **mark who is blind**, so the
//! operator knows that quoting one debater at another ends the control — the same reason
//! `voiceconfirm` puts the consequence on the dialog rather than in a manual.
//!
//! ## Activity is less useful than disagreement
//!
//! Four slots all reading plausibly is the state that needs no attention. What is worth surfacing is
//! where they diverged, because that is the whole reason a panel was run.

/// What a debater is doing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Generating.
    Writing,
    /// Finished this round.
    Done,
    /// Wants operator input; `panelvoice::triage` decides whether it gets asked.
    Asked,
    /// Held back this round as a control.
    Blind,
}

impl State {
    /// A short glyph for the strip. Blind is marked distinctly because it changes what the
    /// operator's own words will do.
    pub fn glyph(self) -> &'static str {
        match self { State::Writing => "…", State::Done => "✓", State::Asked => "?", State::Blind => "◯" }
    }
}

/// One debater's row.
#[derive(Clone, PartialEq, Debug)]
pub struct Slot {
    pub model: String,
    pub state: State,
    /// The last COMPLETE line, never a partial one.
    pub last_line: String,
    /// True when this debater has not seen the operator's input this round.
    pub blind: bool,
}

/// Longest line the strip shows before eliding.
///
/// A strip is read at a glance, so a slot that wraps stops being glanceable and starts competing with
/// the transcript for attention.
pub const MAX_LINE_CHARS: usize = 72;

/// Extract the last complete line from a partial stream.
///
/// Returns None while the first line is still being written — showing a fragment would reintroduce
/// exactly the jitter this avoids.
pub fn last_complete_line(buf: &str) -> Option<&str> {
    let end = buf.rfind(['.', '!', '?', '\n'])?;
    let head = &buf[..=end];
    let start = head[..end].rfind(['.', '!', '?', '\n']).map(|i| i + 1).unwrap_or(0);
    let line = head[start..].trim();
    if line.is_empty() { None } else { Some(line) }
}

/// Shorten for the strip, from the END rather than the start.
///
/// The most recent words matter most in a live view — a line elided from the right shows how the
/// thought began and hides where it got to, which is backwards for something being watched.
pub fn elide(line: &str, max: usize) -> String {
    let n = line.chars().count();
    if n <= max { return line.to_string(); }
    let tail: String = line.chars().skip(n - max + 1).collect();
    format!("…{tail}")
}

/// Do these debaters currently disagree?
///
/// The state worth surfacing; unanimity is the state that needs no attention.
pub fn diverged(slots: &[Slot]) -> bool {
    let done: Vec<&Slot> = slots.iter().filter(|s| s.state == State::Done).collect();
    if done.len() < 2 { return false; }
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let first = norm(&done[0].last_line);
    done.iter().any(|s| norm(&s.last_line) != first)
}

/// The warning shown when a blind debater is on screen.
pub const BLIND_NOTE: &str = "\
◯ is blind this round — it has not seen your input. Quoting another debater at it carries the \
information across by hand and ends the control, so the next round can no longer tell a corroborated \
answer from your hint arriving twice.";

/// Render the strip. Returns true if anything was drawn.
///
/// Compact by design: it sits beside the transcript and must not compete with it for attention. One
/// row per debater, glyph then model then the last complete line, and the blind note ONLY when a blind
/// member is present — a permanent warning is furniture and stops being read.
#[cfg(feature = "gui")]
pub fn ui(ui: &mut egui::Ui, slots: &[Slot]) -> bool {
    use egui::RichText;
    if slots.is_empty() { return false; }
    if diverged(slots) {
        ui.label(RichText::new("⚡ they disagree — this is what the panel was run for")
            .small().color(crate::theme::AMBER()));
    }
    for s in slots {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(s.state.glyph()).small().monospace());
            ui.label(RichText::new(&s.model).small().color(crate::theme::DIM()));
        });
        if !s.last_line.is_empty() {
            ui.label(RichText::new(elide(&s.last_line, MAX_LINE_CHARS))
                .small().color(crate::theme::FAINT()));
        }
    }
    if slots.iter().any(|s| s.blind) {
        ui.label(RichText::new(BLIND_NOTE).small().color(crate::theme::DIM()));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(m: &str, st: State, line: &str, blind: bool) -> Slot {
        Slot { model: m.into(), state: st, last_line: line.into(), blind }
    }

    #[test]
    fn a_partial_first_line_shows_NOTHING() {
        // Four simultaneous token streams is four jittering edges and a person can read none of them.
        // Showing a fragment reintroduces exactly that.
        assert_eq!(last_complete_line("I think we should use a Bt"), None);
        assert_eq!(last_complete_line(""), None);
    }

    #[test]
    fn the_LAST_complete_line_is_shown_not_the_first() {
        let buf = "First I considered a HashMap. Then I realised ordering matters. Now I am writing";
        assert_eq!(last_complete_line(buf), Some("Then I realised ordering matters."));
    }

    #[test]
    fn elision_keeps_the_END_of_the_line() {
        // The most recent words matter most in a live view. Eliding from the right would show how the
        // thought began and hide where it got to, which is backwards for something being watched.
        let long = "a".repeat(50) + " and the conclusion is here";
        let e = elide(&long, 20);
        assert!(e.starts_with('…'));
        assert!(e.ends_with("conclusion is here"), "got {e}");
        assert_eq!(e.chars().count(), 20);
    }

    #[test]
    fn a_short_line_is_left_alone() {
        assert_eq!(elide("brief", MAX_LINE_CHARS), "brief");
    }

    #[test]
    fn divergence_is_detected_only_among_FINISHED_debaters() {
        // Two half-written turns will differ from each other trivially. Comparing them would show
        // "diverged" constantly and mean nothing.
        let mid = vec![s("a", State::Writing, "one thing", false),
                       s("b", State::Writing, "another", false)];
        assert!(!diverged(&mid));
        let done = vec![s("a", State::Done, "use a BTreeMap", false),
                        s("b", State::Done, "use a HashMap", false)];
        assert!(diverged(&done));
    }

    #[test]
    fn unanimity_is_NOT_flagged() {
        // The state that needs no attention. Flagging it would make the indicator meaningless.
        let all = vec![s("a", State::Done, "use a BTreeMap", false),
                       s("b", State::Done, "Use a  btreemap", false)];
        assert!(!diverged(&all));
    }

    #[test]
    fn a_blind_debater_is_marked_and_the_reason_is_stated() {
        // The panel cannot stop an operator carrying A's reasoning to B by hand, and should not try.
        // It can say that doing so ends the control — the same reason voiceconfirm puts the
        // consequence on the dialog rather than in a manual.
        assert_eq!(State::Blind.glyph(), "◯");
        assert!(BLIND_NOTE.contains("carries the information across by hand"));
        assert!(BLIND_NOTE.contains("ends the control"));
    }
}
