//! speakers — who said what, and showing only one of them.
//!
//! ## What a backend can know and a heuristic cannot
//!
//! Real diarisation compares **voice embeddings**: it hears that the voice at
//! 4:10 is the same one from 0:30 and labels both Speaker A. That needs a model
//! and it is the only way to identify a speaker who leaves and returns.
//!
//! Without one, the audio still offers a signal — a pause long enough to be a
//! turn boundary. But that signal answers a strictly weaker question. It can say
//! *"the speaker probably changed here"*; it cannot say *"this is the person
//! from earlier"*. So:
//!
//! * [`Source::Model`] labels are identities. Speaker A at 4:10 really is
//!   Speaker A from 0:30, and filtering to A gives everything they said.
//! * [`Source::Turns`] labels are **alternation**, and are named `Turn 1`,
//!   `Turn 2` rather than `Speaker A`, `Speaker B`. A transcript that called
//!   them speakers would be claiming something it cannot support — and a user
//!   filtering "Speaker A" out of a two-hour meeting would silently lose half
//!   of what that person said.
//!
//! Encoding that distinction in the type is the point of this module. A UI can
//! offer filtering on model labels and offer turn markers on heuristic ones,
//! and cannot accidentally offer the wrong affordance.

use crate::audio::Segment;

/// Where a speaker label came from, which decides what it means.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// A diarisation model compared voices. Labels are identities.
    Model,
    /// Pause-based turn detection. Labels are alternation, not identity.
    Turns,
}

impl Source {
    /// May a caller filter a whole transcript by this label?
    ///
    /// Only for model labels. Filtering on turn numbers looks like it works and
    /// returns an arbitrary subset.
    pub fn is_identity(self) -> bool { matches!(self, Source::Model) }
}

/// A speaker label attached to a segment.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Speaker {
    /// Stable within one transcript. `0` is the first speaker heard.
    pub id: usize,
    pub source: Source,
    /// A name the user typed, if any. Overrides the generated label.
    pub name: Option<String>,
}

impl Speaker {
    pub fn new(id: usize, source: Source) -> Speaker {
        Speaker { id, source, name: None }
    }

    /// How it reads.
    ///
    /// Letters for identities and numbers for turns, so the two are
    /// distinguishable at a glance in a transcript with no legend.
    pub fn label(&self) -> String {
        if let Some(n) = &self.name {
            if !n.trim().is_empty() { return n.trim().to_string(); }
        }
        match self.source {
            Source::Model => {
                // A..Z then A2, B2 — rather than wrapping to A, which would
                // silently merge two speakers in a crowded recording.
                let letter = (b'A' + (self.id % 26) as u8) as char;
                let cycle = self.id / 26;
                if cycle == 0 { format!("Speaker {letter}") }
                else { format!("Speaker {letter}{}", cycle + 1) }
            }
            Source::Turns => format!("Turn {}", self.id + 1),
        }
    }
}

/// A transcript with speaker annotations.
#[derive(Clone, Debug, Default)]
pub struct Diarised {
    pub segments: Vec<Segment>,
    /// Parallel to `segments`; `None` where nothing could be determined.
    pub speakers: Vec<Option<Speaker>>,
}

/// Silence long enough to count as a turn boundary.
///
/// 700ms. Below it, ordinary within-sentence pauses split a single speaker into
/// dozens of turns; well above it, a brisk exchange reads as one person. It is a
/// heuristic constant and the type says so — which is why nothing downstream
/// treats its output as identity.
pub const TURN_GAP_MS: u64 = 700;

/// Label turns by pause length.
///
/// Deliberately alternating rather than clustering: with no voice information,
/// the only defensible claim is "a boundary happened here", and alternating A/B
/// would imply exactly the two-speaker identity assumption this cannot make.
/// Each turn gets its own number.
pub fn label_turns(segments: &[Segment]) -> Vec<Option<Speaker>> {
    let mut out = Vec::with_capacity(segments.len());
    let mut turn = 0usize;
    let mut prev_end: Option<u64> = None;
    for s in segments {
        if let Some(p) = prev_end {
            // Saturating: segments can arrive out of order from a backend that
            // parallelised, and a negative gap must not wrap into an enormous
            // one and start a turn on every segment.
            if s.start_ms.saturating_sub(p) >= TURN_GAP_MS { turn += 1; }
        }
        out.push(Some(Speaker::new(turn, Source::Turns)));
        prev_end = Some(s.end_ms.max(s.start_ms));
    }
    out
}

impl Diarised {
    pub fn new(segments: Vec<Segment>, speakers: Vec<Option<Speaker>>) -> Diarised {
        let mut speakers = speakers;
        // Length mismatch is a backend bug, and truncating silently would
        // mislabel every segment after the mismatch. Padding with None labels
        // them "unknown", which is true.
        speakers.resize(segments.len(), None);
        Diarised { segments, speakers }
    }

    /// From pause-based turn detection alone.
    pub fn from_turns(segments: Vec<Segment>) -> Diarised {
        let speakers = label_turns(&segments);
        Diarised { segments, speakers }
    }

    /// Distinct speakers, in the order first heard.
    ///
    /// First-heard rather than sorted by id, because a transcript reads top to
    /// bottom and a legend ordered otherwise sends the reader hunting.
    pub fn roster(&self) -> Vec<Speaker> {
        let mut seen: Vec<Speaker> = Vec::new();
        for sp in self.speakers.iter().flatten() {
            if !seen.iter().any(|s| s.id == sp.id) { seen.push(sp.clone()); }
        }
        seen
    }

    /// Can this transcript be filtered by speaker?
    ///
    /// Three conditions, and the third is the one I got wrong first: `flatten()`
    /// skips unlabelled segments, so a transcript where a backend labelled half
    /// the segments passed as filterable — and filtering it would silently drop
    /// every unlabelled line with no sign anything was missing. An UNKNOWN
    /// speaker is not a speaker you can filter around.
    pub fn filterable(&self) -> bool {
        !self.segments.is_empty()
            && self.speakers.iter().all(|s| s.as_ref().map(|s| s.source.is_identity())
                                             .unwrap_or(false))
            && !self.roster().is_empty()
    }

    /// Segments for one speaker.
    ///
    /// Returns indices rather than clones so a caller can highlight in place
    /// rather than build a second transcript that drifts from the first.
    pub fn indices_for(&self, id: usize) -> Vec<usize> {
        self.speakers.iter().enumerate()
            .filter_map(|(i, s)| s.as_ref().filter(|s| s.id == id).map(|_| i))
            .collect()
    }

    /// Rename a speaker everywhere.
    ///
    /// Applied to every segment rather than stored in a side table, so an
    /// export cannot disagree with the screen about who is who.
    pub fn rename(&mut self, id: usize, name: &str) {
        for s in self.speakers.iter_mut().flatten() {
            if s.id == id {
                s.name = if name.trim().is_empty() { None } else { Some(name.trim().into()) };
            }
        }
    }

    /// Raw text, optionally for one speaker only.
    ///
    /// `only` on a non-filterable transcript returns everything rather than an
    /// arbitrary slice: silently honouring it would be the failure the `Source`
    /// distinction exists to prevent.
    pub fn raw_text(&self, only: Option<usize>) -> String {
        let filtering = only.filter(|_| self.filterable());
        self.segments.iter().enumerate()
            .filter(|(i, _)| match filtering {
                Some(id) => self.speakers[*i].as_ref().map(|s| s.id == id).unwrap_or(false),
                None => true,
            })
            .map(|(_, s)| s.text.trim())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Text with a speaker prefix per line, the way a transcript reads.
    ///
    /// The prefix is emitted only when the speaker CHANGES, because repeating
    /// it on every line of a long turn buries the words in labels.
    pub fn annotated_text(&self) -> String {
        let mut out = String::new();
        let mut last: Option<usize> = None;
        for (i, seg) in self.segments.iter().enumerate() {
            if let Some(sp) = &self.speakers[i] {
                if last != Some(sp.id) {
                    if !out.is_empty() { out.push('\n'); }
                    out.push_str(&sp.label());
                    out.push_str(":\n");
                    last = Some(sp.id);
                }
            }
            out.push_str(seg.text.trim());
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: u64, end: u64, text: &str) -> Segment {
        Segment { start_ms: start, end_ms: end, text: text.into(), score: 0.9 }
    }

    fn conversation() -> Vec<Segment> {
        vec![
            seg(0, 1_000, "hello there"),
            seg(1_100, 2_000, "how are you"),      // 100ms — same turn
            seg(3_000, 4_000, "quite well thanks"), // 1000ms — new turn
            seg(5_000, 6_000, "glad to hear it"),   // 1000ms — new turn
        ]
    }

    #[test]
    fn a_short_pause_does_not_start_a_new_turn() {
        // Below the threshold, ordinary within-sentence pauses would split one
        // speaker into dozens of turns.
        let d = Diarised::from_turns(conversation());
        assert_eq!(d.speakers[0].as_ref().unwrap().id, 0);
        assert_eq!(d.speakers[1].as_ref().unwrap().id, 0, "100ms is not a turn");
        assert_eq!(d.speakers[2].as_ref().unwrap().id, 1);
        assert_eq!(d.speakers[3].as_ref().unwrap().id, 2);
    }

    #[test]
    fn turn_labels_are_numbers_and_model_labels_are_letters() {
        // Distinguishable at a glance in a transcript with no legend, because
        // they mean different things.
        assert_eq!(Speaker::new(0, Source::Turns).label(), "Turn 1");
        assert_eq!(Speaker::new(0, Source::Model).label(), "Speaker A");
        assert_eq!(Speaker::new(1, Source::Model).label(), "Speaker B");
    }

    #[test]
    fn a_crowded_recording_does_not_wrap_two_speakers_onto_one_letter() {
        // Wrapping at Z would silently merge two people.
        assert_eq!(Speaker::new(25, Source::Model).label(), "Speaker Z");
        assert_eq!(Speaker::new(26, Source::Model).label(), "Speaker A2");
        assert_ne!(Speaker::new(0, Source::Model).label(),
                   Speaker::new(26, Source::Model).label());
    }

    #[test]
    fn a_turn_labelled_transcript_is_not_filterable() {
        // The whole point of the Source distinction: filtering on turn numbers
        // looks like it works and returns an arbitrary subset. A user filtering
        // "Speaker A" out of a two-hour meeting would silently lose half of
        // what that person said.
        let d = Diarised::from_turns(conversation());
        assert!(!d.filterable());
    }

    #[test]
    fn a_model_labelled_transcript_is_filterable_and_filters_correctly() {
        let segs = conversation();
        let speakers = vec![
            Some(Speaker::new(0, Source::Model)),
            Some(Speaker::new(1, Source::Model)),
            Some(Speaker::new(0, Source::Model)),
            Some(Speaker::new(1, Source::Model)),
        ];
        let d = Diarised::new(segs, speakers);
        assert!(d.filterable());
        assert_eq!(d.indices_for(0), vec![0, 2]);
        assert_eq!(d.raw_text(Some(0)), "hello there\nquite well thanks");
    }

    #[test]
    fn filtering_a_non_filterable_transcript_returns_everything() {
        // Silently honouring it would be exactly the failure the Source
        // distinction exists to prevent.
        let d = Diarised::from_turns(conversation());
        assert_eq!(d.raw_text(Some(0)), d.raw_text(None));
        assert_eq!(d.raw_text(Some(0)).lines().count(), 4);
    }

    #[test]
    fn a_speaker_who_leaves_and_returns_is_one_speaker_under_a_model() {
        // The thing turn detection structurally cannot do, and the reason the
        // two sources are different types rather than a flag.
        let segs = vec![seg(0, 1_000, "first"), seg(60_000, 61_000, "later"),
                        seg(120_000, 121_000, "much later")];
        let model = Diarised::new(segs.clone(), vec![
            Some(Speaker::new(0, Source::Model)),
            Some(Speaker::new(1, Source::Model)),
            Some(Speaker::new(0, Source::Model)),
        ]);
        assert_eq!(model.roster().len(), 2);
        assert_eq!(model.indices_for(0), vec![0, 2]);

        let turns = Diarised::from_turns(segs);
        assert_eq!(turns.roster().len(), 3, "turn detection sees three turns, not two people");
    }

    #[test]
    fn the_roster_is_in_the_order_first_heard() {
        // A transcript reads top to bottom; a legend ordered otherwise sends
        // the reader hunting.
        let d = Diarised::new(conversation(), vec![
            Some(Speaker::new(2, Source::Model)),
            Some(Speaker::new(0, Source::Model)),
            Some(Speaker::new(2, Source::Model)),
            Some(Speaker::new(1, Source::Model)),
        ]);
        assert_eq!(d.roster().iter().map(|s| s.id).collect::<Vec<_>>(), vec![2, 0, 1]);
    }

    #[test]
    fn renaming_applies_everywhere_so_an_export_cannot_disagree_with_the_screen() {
        let mut d = Diarised::new(conversation(), vec![
            Some(Speaker::new(0, Source::Model)); 4]);
        d.rename(0, "  Ada  ");
        assert_eq!(d.roster()[0].label(), "Ada", "trimmed");
        assert!(d.annotated_text().starts_with("Ada:"));
        d.rename(0, "   ");
        assert_eq!(d.roster()[0].label(), "Speaker A", "blank clears rather than sets an empty name");
    }

    #[test]
    fn the_speaker_prefix_appears_only_when_the_speaker_changes() {
        // Repeating it on every line of a long turn buries the words in labels.
        let d = Diarised::from_turns(conversation());
        let text = d.annotated_text();
        assert_eq!(text.matches("Turn 1:").count(), 1);
        assert!(text.contains("hello there\nhow are you"), "one turn runs on: {text}");
    }

    #[test]
    fn segments_arriving_out_of_order_do_not_start_a_turn_on_every_line() {
        // A backend that parallelised can emit these; a negative gap wrapping
        // on unsigned arithmetic would make every segment its own turn.
        let segs = vec![seg(5_000, 6_000, "later"), seg(0, 1_000, "earlier"),
                        seg(6_100, 7_000, "after")];
        let d = Diarised::from_turns(segs);
        assert!(d.roster().len() < 3, "out-of-order did not fragment: {:?}", d.roster());
    }

    #[test]
    fn a_backend_returning_too_few_labels_marks_the_rest_unknown() {
        // Truncating silently would mislabel every segment after the mismatch.
        let d = Diarised::new(conversation(), vec![Some(Speaker::new(0, Source::Model))]);
        assert_eq!(d.speakers.len(), 4);
        assert!(d.speakers[3].is_none());
        assert!(!d.filterable(), "unknown labels make it unfilterable");
    }

    #[test]
    fn an_empty_transcript_has_no_roster_and_is_not_filterable() {
        let d = Diarised::from_turns(vec![]);
        assert!(d.roster().is_empty());
        assert!(!d.filterable(), "nothing to filter by");
        assert_eq!(d.raw_text(None), "");
    }
}
