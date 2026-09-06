//! zk_share — share the zettelkasten as a HUGE QR code: pick the notes, paint what is
//! public, pass two gates, show the code (or the sequence of codes).
//!
//! The deck is the zettelkasten (`cards_view::Deck` over `note_cards::Card`). Nothing in it
//! is public by default: a note is a thing you wrote to yourself. Sharing is three steps,
//! each one a screen, and the model is the sequence:
//!
//! 1. **Pick.** Every note is a row with a checkbox; the sections (headings) group them
//!    and a section's box picks its notes together. The **paintbrush** is the fast way:
//!    with the brush down, dragging over rows marks them public (or private, with the
//!    other brush) instead of tapping each box. `public` is the note's own flag and
//!    survives the picker — it is a property of the note, not of this share.
//! 2. **Gate one — what.** "Share N notes, K bytes, as M codes?" with the titles listed.
//!    Cancel goes back to the picker with nothing lost.
//! 3. **Gate two — who.** "Anyone who scans can read them. The code is not encrypted."
//!    This is the one that matters, so it is its own screen rather than a line in the
//!    first: a person nodding through "N notes" should still have to say yes to
//!    "anyone".
//! 4. **Show.** The kit's huge view, cycling frames when there is more than one.
//!
//! The payload is plain text — `# title` / body, one blank line between — so a scanner
//! without this app still gets a readable page, and the frame headers are the kit's
//! (`qrbig::frames`). No clock, no drawing; the shell draws it (the gate dialogs are built
//! here as values, drawn there).

use crate::cards_view::Card;

/// One note as the picker shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Index into the deck's cards.
    pub index: usize,
    pub title: String,
    pub is_heading: bool,
    pub bytes: usize,
    /// Marked public — by its box, or by the brush.
    pub public: bool,
}

/// Which brush is down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Brush {
    #[default]
    Off,
    /// Dragging over rows marks them public.
    Public,
    /// Dragging over rows marks them private.
    Private,
}

/// Where the share is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Pick,
    /// Gate one: what goes out.
    ConfirmWhat,
    /// Gate two: who can read it.
    ConfirmWho,
    /// The code is up.
    Showing,
}

#[derive(Debug, Clone)]
pub struct Share {
    pub rows: Vec<Row>,
    pub brush: Brush,
    pub stage: Stage,
}

/// The largest share the picker will let through, in bytes: fifty frames' worth. A
/// zettelkasten past that is a file, not a code — the sequence would take a minute of
/// holding a phone still.
pub const MAX_BYTES: usize = 50 * uikit_egui::qrbig::FRAME_PAYLOAD;

impl Share {
    /// From the deck: every card a row, nothing public.
    pub fn from_cards(cards: &[Card]) -> Share {
        let rows = cards
            .iter()
            .enumerate()
            .map(|(i, c)| Row {
                index: i,
                title: if c.title.trim().is_empty() { format!("note {}", i + 1) } else { c.title.clone() },
                is_heading: matches!(c.kind, crate::cards_view::CardKind::Heading),
                bytes: c.title.len() + c.body.len() + 4,
                public: false,
            })
            .collect();
        Share { rows, brush: Brush::Off, stage: Stage::Pick }
    }

    /// Tap a row's box.
    pub fn toggle(&mut self, index: usize) {
        if let Some(r) = self.rows.iter_mut().find(|r| r.index == index) {
            r.public = !r.public;
        }
    }

    /// A heading's box picks the heading and every row under it, up to the next heading.
    pub fn set_section(&mut self, heading_index: usize, public: bool) {
        let Some(start) = self.rows.iter().position(|r| r.index == heading_index) else { return };
        for (i, r) in self.rows.iter_mut().enumerate().skip(start) {
            if i > start && r.is_heading {
                break;
            }
            r.public = public;
        }
    }

    /// The brush passes over a row: it takes the brush's mark. Nothing with the brush up.
    pub fn brush_over(&mut self, index: usize) {
        let mark = match self.brush {
            Brush::Off => return,
            Brush::Public => true,
            Brush::Private => false,
        };
        if let Some(r) = self.rows.iter_mut().find(|r| r.index == index) {
            r.public = mark;
        }
    }

    pub fn public_rows(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| r.public)
    }
    pub fn public_count(&self) -> usize {
        self.public_rows().count()
    }
    pub fn public_bytes(&self) -> usize {
        self.public_rows().map(|r| r.bytes).sum()
    }
    /// How many codes the share takes.
    pub fn frame_count(&self) -> usize {
        self.public_bytes().div_ceil(uikit_egui::qrbig::FRAME_PAYLOAD).max(1)
    }
    /// Why the share cannot go on, if it cannot.
    pub fn blocker(&self) -> Option<String> {
        if self.public_count() == 0 {
            return Some("nothing is marked public".into());
        }
        if self.public_bytes() > MAX_BYTES {
            return Some(format!(
                "{} KB is too much for a code sequence — export a file instead",
                self.public_bytes() / 1024
            ));
        }
        None
    }

    /// The payload: the public notes as plain text, in deck order.
    pub fn payload(&self, cards: &[Card]) -> Vec<u8> {
        let mut out = String::new();
        for r in self.public_rows() {
            let Some(c) = cards.get(r.index) else { continue };
            if !out.is_empty() {
                out.push('\n');
            }
            if r.is_heading {
                out.push_str("# ");
                out.push_str(c.title.trim());
                out.push('\n');
            } else {
                out.push_str("## ");
                out.push_str(c.title.trim());
                out.push('\n');
            }
            let body = c.body.trim();
            if !body.is_empty() && body != c.title.trim() {
                out.push_str(body);
                out.push('\n');
            }
        }
        out.into_bytes()
    }

    // ── the stages ──

    /// "Share" pressed on the picker: to gate one, unless something blocks.
    pub fn go(&mut self) -> Result<(), String> {
        if self.stage != Stage::Pick {
            return Ok(());
        }
        match self.blocker() {
            Some(b) => Err(b),
            None => {
                self.stage = Stage::ConfirmWhat;
                Ok(())
            }
        }
    }
    /// Gate one confirmed: to gate two.
    pub fn what_confirmed(&mut self) {
        if self.stage == Stage::ConfirmWhat {
            self.stage = Stage::ConfirmWho;
        }
    }
    /// Gate two confirmed: show. Only from gate two — no route skips it.
    pub fn who_confirmed(&mut self) {
        if self.stage == Stage::ConfirmWho {
            self.stage = Stage::Showing;
        }
    }
    /// Cancel at either gate: back to the picker, the marks kept.
    pub fn back(&mut self) {
        self.stage = Stage::Pick;
    }

    /// Gate one's text.
    pub fn what_text(&self) -> String {
        let n = self.public_count();
        let kb = self.public_bytes() as f32 / 1024.0;
        let frames = self.frame_count();
        let mut titles: Vec<&str> = self.public_rows().map(|r| r.title.as_str()).take(8).collect();
        let more = n.saturating_sub(titles.len());
        if more > 0 {
            titles.push("…");
        }
        format!(
            "{n} note{} · {kb:.1} KB · {} code{}\n\n{}{}",
            if n == 1 { "" } else { "s" },
            frames,
            if frames == 1 { "" } else { "s" },
            titles.join("\n"),
            if more > 0 { format!("\n(and {more} more)") } else { String::new() }
        )
    }
    /// The gate for the current stage, as the kit's dialog — built here so the binary has
    /// exactly one `ConfirmDialog::new` site of its own (the terminal's risk gate, which a
    /// test counts). Gate two times out to Cancel after a minute: a code left on a screen
    /// with nobody deciding should not appear on its own.
    pub fn gate(&self) -> Option<uikit_egui::confirm_dialog::ConfirmDialog> {
        use uikit_egui::confirm_dialog::{ConfirmDialog, TimeoutAction};
        match self.stage {
            // 45 s, and CANCEL rather than the dialog's 10-second Discard default: gate one
            // lists the titles to read, and a reader who takes eleven seconds should find
            // the picker again, not a share that quietly dropped out of the flow. Seen
            // under Xvfb — the countdown was already at 9 s in the first screenshot.
            Stage::ConfirmWhat => Some(
                ConfirmDialog::new("Share these notes?", self.what_text(), "Continue")
                    .with_timeout(45.0)
                    .with_timeout_action(TimeoutAction::Cancel),
            ),
            Stage::ConfirmWho => Some(
                ConfirmDialog::new("Anyone can read them", self.who_text(), "Show the code")
                    .with_timeout(60.0)
                    .with_timeout_action(TimeoutAction::Cancel),
            ),
            _ => None,
        }
    }

    /// Gate two's text.
    pub fn who_text(&self) -> &'static str {
        "Anyone who scans the code can read these notes.\n\
         The code is not encrypted and cannot be taken back once it has been seen.\n\n\
         Show it?"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards_view::{CardKind, CardSource};

    fn card(title: &str, body: &str, kind: CardKind) -> Card {
        Card { title: title.into(), body: body.into(), kind, source: CardSource { url: String::new(), paragraph_id: String::new() } }
    }

    fn deck() -> Vec<Card> {
        vec![
            card("Gardens", "", CardKind::Heading),
            card("Compost", "Turn it weekly.", CardKind::Prose),
            card("Mulch", "Straw over the beds.", CardKind::Prose),
            card("Kitchen", "", CardKind::Heading),
            card("Sourdough", "Feed the starter.", CardKind::Prose),
        ]
    }

    #[test]
    fn nothing_is_public_until_marked_and_the_gates_go_in_order_with_no_skip() {
        let cards = deck();
        let mut s = Share::from_cards(&cards);
        assert_eq!(s.public_count(), 0);
        assert!(s.go().is_err(), "nothing public: blocked at the picker");
        s.toggle(1);
        assert_eq!(s.public_count(), 1);
        s.who_confirmed();
        assert_eq!(s.stage, Stage::Pick, "gate two cannot be taken from the picker");
        s.what_confirmed();
        assert_eq!(s.stage, Stage::Pick, "nor gate one without Share");
        assert!(s.go().is_ok());
        assert_eq!(s.stage, Stage::ConfirmWhat);
        s.who_confirmed();
        assert_eq!(s.stage, Stage::ConfirmWhat, "gate two is not gate one");
        s.what_confirmed();
        assert_eq!(s.stage, Stage::ConfirmWho);
        s.back();
        assert_eq!(s.stage, Stage::Pick);
        assert_eq!(s.public_count(), 1, "cancel keeps the marks");
        s.go().unwrap();
        s.what_confirmed();
        s.who_confirmed();
        assert_eq!(s.stage, Stage::Showing);
    }

    #[test]
    fn a_section_box_picks_its_notes_and_the_brush_marks_what_it_passes_over() {
        let cards = deck();
        let mut s = Share::from_cards(&cards);
        s.set_section(0, true);
        assert_eq!(s.public_rows().map(|r| r.index).collect::<Vec<_>>(), vec![0, 1, 2], "the heading and its two notes, not the next section");
        s.brush = Brush::Private;
        s.brush_over(2);
        assert!(!s.rows[2].public);
        s.brush = Brush::Off;
        s.brush_over(4);
        assert!(!s.rows[4].public, "the brush up marks nothing");
        s.brush = Brush::Public;
        s.brush_over(4);
        assert!(s.rows[4].public);
    }

    #[test]
    fn the_payload_is_readable_text_in_deck_order_and_the_gate_text_counts_it() {
        let cards = deck();
        let mut s = Share::from_cards(&cards);
        s.set_section(3, true);
        s.toggle(1);
        let text = String::from_utf8(s.payload(&cards)).unwrap();
        assert_eq!(text, "## Compost\nTurn it weekly.\n\n# Kitchen\n\n## Sourdough\nFeed the starter.\n");
        assert_eq!(s.frame_count(), 1);
        let what = s.what_text();
        assert!(what.starts_with("3 notes · 0.1 KB · 1 code\n"), "{what}");
        assert!(what.contains("Compost") && what.contains("Sourdough"));
        assert!(s.who_text().contains("not encrypted"));
    }

    /// A gate that times out must land back at the picker, never onward: the kit's default
    /// is Discard after ten seconds, which for gate one is "you read too slowly, share
    /// cancelled" and for gate two would be the difference between asking and not.
    #[test]
    fn both_gates_time_out_to_cancel_and_give_a_reader_time() {
        use uikit_egui::confirm_dialog::TimeoutAction;
        let cards = deck();
        let mut s = Share::from_cards(&cards);
        s.toggle(1);
        s.go().unwrap();
        let one = s.gate().unwrap();
        assert!(one.timeout_secs >= 30.0, "gate one gives time to read the titles");
        assert_eq!(one.timeout_action, TimeoutAction::Cancel);
        s.what_confirmed();
        let two = s.gate().unwrap();
        assert!(two.timeout_secs >= 30.0);
        assert_eq!(two.timeout_action, TimeoutAction::Cancel);
        s.who_confirmed();
        assert!(s.gate().is_none(), "showing has no gate");
    }

    #[test]
    fn too_much_for_a_sequence_is_blocked_with_the_reason() {
        let big: Vec<Card> = (0..40).map(|i| card(&format!("n{i}"), &"x".repeat(4000), CardKind::Prose)).collect();
        let mut s = Share::from_cards(&big);
        for i in 0..40 {
            s.toggle(i);
        }
        assert!(s.public_bytes() > MAX_BYTES);
        let err = s.go().unwrap_err();
        assert!(err.contains("too much"), "{err}");
        assert_eq!(s.stage, Stage::Pick);
    }
}
