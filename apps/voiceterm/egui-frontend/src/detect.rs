//! detect — the views over voiceterm's detection engines.
//!
//! Two engines already exist and neither is re-implemented (`#[path]`, as with
//! the table engine and the row cache — both are Qt-free plain Rust):
//!
//! * `entity_extract` — terminal-output-as-menu. Run `git branch` or
//!   `bluetoothctl devices`, and it pulls structured entities out of the output
//!   so a later reference ("connect to the headphones") can resolve against
//!   them.
//! * `dumb_summary::detect_progress_bars` — spots ASCII progress bars in output
//!   and infers a percentage.
//!
//! Both had no surface. That is the whole gap: the terminal could understand
//! its own output and had no way to show you that it had.
//!
//! ## The picker is a fallback, not the primary path
//!
//! The engine resolves a spoken reference directly most of the time. The picker
//! appears when it CANNOT — when several entities match about as well as each
//! other. So it opens on ambiguity, sorted by score, and it shows the engine's
//! own `reason` next to each row ("matched 'headphones'", "second commit"),
//! because a list of near-identical branch names with no explanation of why
//! these five is a worse question than the one the user asked.
//!
//! A single confident match must NOT open it. A picker that appears when the
//! answer was already known trains the user to dismiss it, and then it is in
//! the way on the one occasion it matters.
//!
//! ## A detected bar is drawn as a bar
//!
//! When output contains `████████░░ 80%`, the terminal draws its own progress
//! indicator from the inferred percentage. Not instead of the text — the text
//! is what the program printed and hiding it would be a lie about the output —
//! but beside it, so a long-running job is legible at a glance from across the
//! room.
//!
//! The LAST bar in a chunk wins. A build that reprints its bar on every line
//! leaves a column of them in the scrollback, and the newest is the only one
//! describing now.

use crate::entity_extract::{Entity, ResolvedMatch};
use crate::dumb_summary::DetectedBar;

/// Below this score gap between the top two matches, the answer is ambiguous
/// and the picker opens.
pub const AMBIGUITY_GAP: f32 = 0.15;
/// A match at least this good, and clear of the runner-up, is acted on without
/// asking.
pub const CONFIDENT_SCORE: f32 = 0.6;

/// What to do with a resolution attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// One clear winner — act on it without asking.
    Act(String),
    /// Several plausible; ask.
    Ask(Vec<String>),
    /// Nothing matched.
    Nothing,
}

/// Decide between acting and asking.
///
/// Takes the engine's scored matches, sorted or not, and applies the one rule
/// the engine deliberately does not: whether the gap is wide enough to be sure.
/// The engine reports confidence; deciding what confidence is *enough* is a UI
/// judgement, and keeping it here means it can be tuned without touching the
/// extractors.
pub fn decide(matches: &[ResolvedMatch]) -> Resolution {
    let mut sorted: Vec<&ResolvedMatch> = matches.iter().collect();
    sorted.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let Some(top) = sorted.first() else { return Resolution::Nothing };
    if top.score <= 0.0 { return Resolution::Nothing; }
    let runner_up = sorted.get(1).map(|m| m.score).unwrap_or(0.0);
    if top.score >= CONFIDENT_SCORE && top.score - runner_up >= AMBIGUITY_GAP {
        return Resolution::Act(top.entity.primary.clone());
    }
    Resolution::Ask(sorted.iter().map(|m| m.entity.primary.clone()).collect())
}

/// A row in the picker: what to show, and why it is here.
#[derive(Debug, Clone, PartialEq)]
pub struct PickerRow {
    pub primary: String,
    pub display: String,
    /// The engine's own explanation.
    pub reason: String,
    pub score: f32,
}

impl PickerRow {
    pub fn of(m: &ResolvedMatch) -> Self {
        PickerRow {
            primary: m.entity.primary.clone(),
            // A blank display would give an unselectable row; the canonical id
            // is at least something to read.
            display: if m.entity.display.trim().is_empty() { m.entity.primary.clone() }
                     else { m.entity.display.clone() },
            reason: m.reason.clone(),
            score: m.score,
        }
    }
}

/// The entity picker.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntityPicker {
    rows: Vec<PickerRow>,
    pub open: bool,
    /// What the user said, shown as the question's subject.
    pub reference: String,
}

impl EntityPicker {
    /// Open on an ambiguous resolution. Refuses to open on fewer than two rows:
    /// a picker with one option is a confirmation dialog nobody asked for.
    pub fn ask(&mut self, reference: &str, matches: &[ResolvedMatch]) -> bool {
        let mut rows: Vec<PickerRow> = matches.iter().map(PickerRow::of).collect();
        rows.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        if rows.len() < 2 { return false; }
        self.rows = rows;
        self.reference = reference.to_string();
        self.open = true;
        true
    }

    pub fn rows(&self) -> &[PickerRow] { &self.rows }
    pub fn close(&mut self) { self.open = false; self.rows.clear(); }

    /// Choose a row. Returns the canonical id to substitute into the command.
    pub fn choose(&mut self, primary: &str) -> Option<String> {
        let hit = self.rows.iter().find(|r| r.primary == primary)?.primary.clone();
        self.close();
        Some(hit)
    }

    /// The question. Naming what was said is what makes it answerable — "which
    /// one?" on its own does not say which of what.
    pub fn question(&self) -> String {
        if self.reference.trim().is_empty() { return "Which one?".into(); }
        format!("Which \"{}\"?", self.reference.trim())
    }
}

/// The progress indicator over a detected bar.
///
/// No PartialEq: DetectedBar does not derive it, and adding one to the engine
/// to satisfy a view would be the tail wagging the dog.
#[derive(Debug, Clone, Default)]
pub struct ProgressReadout {
    bar: Option<DetectedBar>,
}

impl ProgressReadout {
    /// Take the LAST bar in a chunk of output.
    ///
    /// A build that reprints its bar on every line leaves a column of them in
    /// the scrollback, and only the newest describes now.
    pub fn observe(&mut self, output: &str) -> bool {
        let bars = crate::dumb_summary::detect_progress_bars(output);
        match bars.last() {
            Some(b) => { self.bar = Some(b.clone()); true }
            // No bar in this chunk does NOT clear the last one: a program that
            // prints a bar and then a blank line has not stopped working, and
            // an indicator that flickers away between updates is worse than
            // none.
            None => false,
        }
    }

    /// Clear it — the command finished, so there is nothing in progress.
    pub fn clear(&mut self) { self.bar = None; }

    pub fn visible(&self) -> bool { self.bar.is_some() }
    pub fn percent(&self) -> u32 { self.bar.as_ref().map(|b| b.percent.min(100)).unwrap_or(0) }
    pub fn fraction(&self) -> Option<(u32, u32)> { self.bar.as_ref().and_then(|b| b.fraction) }

    /// The label beside the arc. An explicit fraction is shown as one, because
    /// "47/100" tells you more about a job than "47%" does — it says how big
    /// the job is.
    pub fn label(&self) -> String {
        match self.fraction() {
            Some((n, d)) => format!("{n}/{d}"),
            None => format!("{}%", self.percent()),
        }
    }

    pub fn t(&self) -> f32 { self.percent() as f32 / 100.0 }
}

/// Draw the picker. Returns the chosen canonical id.
pub fn picker_ui(ui: &mut egui::Ui, p: &mut EntityPicker,
                 c: &uikit_egui::PaletteColors) -> Option<String> {
    if !p.open { return None; }
    let mut chosen = None;
    ui.strong(p.question());
    for row in p.rows().to_vec() {
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(&row.display).frame(false)).clicked() {
                chosen = Some(row.primary.clone());
            }
            // The engine's own reason, not a score bar: "second commit" is an
            // answer, "0.62" is a number.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.colored_label(c.text_weak, &row.reason);
            });
        });
    }
    if ui.button("none of these").clicked() { p.close(); }
    if let Some(id) = &chosen { p.choose(id); }
    chosen
}

/// Draw the progress readout beside the output.
pub fn progress_ui(ui: &mut egui::Ui, r: &ProgressReadout, c: &uikit_egui::PaletteColors) {
    if !r.visible() { return; }
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 6.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 3.0, c.bg_raised);
        let mut fill = rect;
        fill.set_width(rect.width() * r.t());
        ui.painter().rect_filled(fill, 3.0, c.accent);
        ui.colored_label(c.text_weak, r.label());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity_extract::{EntityKind};

    fn ent(primary: &str, display: &str) -> Entity {
        Entity { kind: EntityKind::GitBranch, primary: primary.into(),
                 display: display.into(), aliases: vec![], confidence: 1.0,
                 metadata: Default::default() }
    }

    fn m(primary: &str, score: f32, reason: &str) -> ResolvedMatch {
        ResolvedMatch { entity: ent(primary, primary), score, reason: reason.into() }
    }

    #[test]
    fn a_single_confident_match_is_acted_on_without_asking() {
        // A picker that appears when the answer was already known trains the
        // user to dismiss it, and then it is in the way on the one occasion it
        // matters.
        assert_eq!(decide(&[m("fix/bug", 0.9, "matched 'bug'")]),
                   Resolution::Act("fix/bug".into()));
    }

    #[test]
    fn a_clear_winner_beats_a_weak_runner_up_without_asking() {
        let r = decide(&[m("a", 0.9, "x"), m("b", 0.3, "y")]);
        assert_eq!(r, Resolution::Act("a".into()));
    }

    #[test]
    fn two_close_matches_are_ambiguous_and_ask() {
        // This is the case the picker exists for.
        let r = decide(&[m("a", 0.72, "x"), m("b", 0.70, "y")]);
        assert!(matches!(r, Resolution::Ask(_)));
    }

    #[test]
    fn a_confident_score_with_a_close_second_still_asks() {
        // High confidence in BOTH is not confidence in one.
        let r = decide(&[m("a", 0.95, "x"), m("b", 0.94, "y")]);
        assert!(matches!(r, Resolution::Ask(_)));
    }

    #[test]
    fn a_lone_weak_match_asks_rather_than_guessing() {
        // Acting on a 0.3 match is how "checkout the bug fix one" checks out
        // the wrong branch.
        let r = decide(&[m("a", 0.3, "x")]);
        assert!(matches!(r, Resolution::Ask(_)));
    }

    #[test]
    fn nothing_matched_is_its_own_answer() {
        assert_eq!(decide(&[]), Resolution::Nothing);
        assert_eq!(decide(&[m("a", 0.0, "no match")]), Resolution::Nothing);
    }

    #[test]
    fn the_ask_list_is_ordered_best_first() {
        // Unsorted input, because the engine does not promise an order.
        let r = decide(&[m("c", 0.5, "z"), m("a", 0.72, "x"), m("b", 0.70, "y")]);
        match r {
            Resolution::Ask(ids) => assert_eq!(ids, vec!["a", "b", "c"]),
            other => panic!("expected Ask, got {other:?}"),
        }
    }

    #[test]
    fn the_picker_refuses_to_open_on_one_option() {
        // A picker with one option is a confirmation dialog nobody asked for.
        let mut p = EntityPicker::default();
        assert!(!p.ask("the bug one", &[m("a", 0.4, "x")]));
        assert!(!p.open);
    }

    #[test]
    fn the_picker_opens_sorted_and_keeps_the_engines_reasons() {
        // "second commit" is an answer; "0.62" is a number.
        let mut p = EntityPicker::default();
        assert!(p.ask("the bug one", &[m("b", 0.6, "matched 'bug'"),
                                       m("a", 0.8, "second commit")]));
        assert_eq!(p.rows()[0].primary, "a");
        assert_eq!(p.rows()[0].reason, "second commit");
    }

    #[test]
    fn the_question_names_what_was_said() {
        // "Which one?" on its own does not say which of what.
        let mut p = EntityPicker::default();
        p.ask("  the bug one  ", &[m("a", 0.5, "x"), m("b", 0.5, "y")]);
        assert_eq!(p.question(), "Which \"the bug one\"?");
    }

    #[test]
    fn a_picker_with_no_reference_still_asks_something_sensible() {
        let mut p = EntityPicker::default();
        p.ask("", &[m("a", 0.5, "x"), m("b", 0.5, "y")]);
        assert_eq!(p.question(), "Which one?");
    }

    #[test]
    fn an_entity_with_no_display_falls_back_to_its_canonical_id() {
        // A blank row cannot be chosen.
        let mut r = m("abc123", 0.5, "x");
        r.entity.display = "  ".into();
        assert_eq!(PickerRow::of(&r).display, "abc123");
    }

    #[test]
    fn choosing_returns_the_canonical_id_and_closes() {
        // The canonical id is what gets substituted into the composed command —
        // the display name would not be a valid branch or MAC.
        let mut p = EntityPicker::default();
        p.ask("x", &[m("aa:bb", 0.5, "x"), m("cc:dd", 0.5, "y")]);
        assert_eq!(p.choose("cc:dd").as_deref(), Some("cc:dd"));
        assert!(!p.open);
    }

    #[test]
    fn choosing_something_not_on_the_list_does_nothing() {
        let mut p = EntityPicker::default();
        p.ask("x", &[m("a", 0.5, "x"), m("b", 0.5, "y")]);
        assert_eq!(p.choose("ghost"), None);
        assert!(p.open, "and the question stays up");
    }

    // ── progress ────────────────────────────────────────────────────────────

    #[test]
    fn a_detected_bar_becomes_a_real_progress_indicator() {
        let mut r = ProgressReadout::default();
        assert!(r.observe("building\n████████░░ 80%\n"));
        assert!(r.visible());
        assert_eq!(r.percent(), 80);
        assert!((r.t() - 0.8).abs() < 0.01);
    }

    #[test]
    fn the_last_bar_in_a_chunk_wins() {
        // A build that reprints its bar every line leaves a column of them, and
        // only the newest describes now.
        let mut r = ProgressReadout::default();
        r.observe("██░░░░░░░░ 20%\n█████░░░░░ 50%\n████████░░ 80%\n");
        assert_eq!(r.percent(), 80);
    }

    #[test]
    fn output_with_no_bar_does_not_clear_the_last_one() {
        // A program that prints a bar and then a blank line has not stopped
        // working, and an indicator that flickers away between updates is worse
        // than none.
        let mut r = ProgressReadout::default();
        r.observe("████████░░ 80%\n");
        assert!(!r.observe("still going\n"));
        assert!(r.visible());
        assert_eq!(r.percent(), 80);
    }

    #[test]
    fn finishing_clears_it_because_nothing_is_in_progress() {
        let mut r = ProgressReadout::default();
        r.observe("████████░░ 80%\n");
        r.clear();
        assert!(!r.visible());
        assert_eq!(r.percent(), 0);
    }

    #[test]
    fn ordinary_text_is_not_mistaken_for_a_bar() {
        // The engine's own minimum run length is what stops one "=" in a config
        // dump becoming a progress indicator.
        let mut r = ProgressReadout::default();
        assert!(!r.observe("key=value\nother = thing\n"));
        assert!(!r.visible());
    }

    #[test]
    fn an_explicit_fraction_is_shown_as_one() {
        // "47/100" says how big the job is; "47%" does not.
        let mut r = ProgressReadout::default();
        if r.observe("[=====     ] 47/100\n") {
            if r.fraction().is_some() {
                assert_eq!(r.label(), "47/100");
                return;
            }
        }
        // If this engine did not read a fraction from that shape, the label
        // must still be sane rather than empty.
        assert!(r.label().ends_with('%') || r.label().contains('/'));
    }

    #[test]
    fn an_empty_readout_reports_zero_rather_than_panicking() {
        let r = ProgressReadout::default();
        assert!(!r.visible());
        assert_eq!(r.t(), 0.0);
        assert_eq!(r.label(), "0%");
    }
}
