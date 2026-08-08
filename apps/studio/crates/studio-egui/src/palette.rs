//! palette — one box for transcript search, commands and routines.
//!
//! `studio-voice` states the design and had no view: *"transcript search, voice
//! commands, and the command palette are all 'type or say a phrase, get the
//! thing you meant'. Building them as one avoids three half-good search
//! boxes."* This is that one box.
//!
//! ## Two kinds of answer, one query, and they must not interleave
//!
//! A query returns routines ("tighten this") and transcript hits ("where did
//! she say margin"). They are ranked by different things and mean different
//! things — one performs an edit, the other moves the playhead — so they are
//! grouped rather than merged into a single ranked list. A palette where the
//! Enter key sometimes edits and sometimes navigates, depending on what floated
//! to the top, is one people stop using at speed.
//!
//! **Routines come first.** They are destructive and few; hits are safe and
//! many. Putting a hundred transcript matches above three commands buries the
//! commands, and the user reaches for a palette precisely when they do not want
//! to hunt.
//!
//! ## Nothing is performed without a selection
//!
//! A `Routine`'s steps carry placeholders the caller fills from context — the
//! selection, the playhead. Selecting an entry does not run it; it returns the
//! choice, and the host decides whether the context is sufficient. A palette
//! that ran "tighten this" with nothing selected would either no-op silently or
//! guess at a target, and both are worse than a refusal that says why.
//!
//! ## Ranking that improves without surprising
//!
//! `Routine::uses` is the usage count the crate calls "the database buildup that
//! makes the palette's ranking get better the more you edit". It breaks ties
//! only — a frequently-used routine does not outrank a better textual match, or
//! the palette stops responding to what you actually typed and starts
//! second-guessing you.

use studio_voice::{Hit, Routine};

/// A ranked palette entry.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// A routine to perform. Carries its index in the repertoire.
    Routine { index: usize, name: String, uses: u32 },
    /// A place to jump to.
    Hit(Hit),
}

impl Entry {
    /// Does choosing this change the project?
    ///
    /// The distinction the UI must make visible: navigation is free, an edit is
    /// not, and a palette that presents them identically trains people to press
    /// Enter without reading.
    pub fn is_destructive(&self) -> bool { matches!(self, Entry::Routine { .. }) }

    pub fn label(&self) -> &str {
        match self {
            Entry::Routine { name, .. } => name,
            Entry::Hit(h) => &h.text,
        }
    }
}

/// How well a phrase matches a query, higher is better. `None` means no match.
///
/// Prefix beats substring beats nothing. Deliberately simple: a fuzzy matcher
/// that scored "tighten" against "the lighting" would put an edit above a
/// navigation on a query that meant neither, and in a palette a surprising
/// match is worse than a missing one.
pub fn score(phrase: &str, query: &str) -> Option<u32> {
    let (p, q) = (phrase.to_lowercase(), query.trim().to_lowercase());
    if q.is_empty() { return None; }
    if p == q { return Some(300); }
    if p.starts_with(&q) { return Some(200); }
    if p.contains(&q) { return Some(100); }
    None
}

/// Build the entry list for a query.
///
/// `hits` come from `TranscriptIndex::find`, which has already mapped them
/// through the clips that reference each source — this must not re-rank them by
/// text, because their order encodes timeline position and that is what the
/// user is navigating by.
pub fn entries(repertoire: &[Routine], hits: Vec<Hit>, query: &str) -> Vec<Entry> {
    let q = query.trim();
    if q.is_empty() { return Vec::new(); }

    let mut routines: Vec<(u32, usize, &Routine)> = repertoire.iter().enumerate()
        .filter_map(|(i, r)| {
            // The best-matching phrase decides: people do not say the same
            // words twice, and a routine should rank by its closest alias
            // rather than its first.
            r.phrases.iter().filter_map(|p| score(p, q)).max().map(|s| (s, i, r))
        })
        .collect();
    // Usage breaks TIES only — see the module docs.
    routines.sort_by(|a, b| b.0.cmp(&a.0)
        .then_with(|| b.2.uses.cmp(&a.2.uses))
        .then_with(|| a.2.name.cmp(&b.2.name)));

    let mut out: Vec<Entry> = routines.into_iter()
        .map(|(_, i, r)| Entry::Routine { index: i, name: r.name.clone(), uses: r.uses })
        .collect();
    out.extend(hits.into_iter().map(Entry::Hit));
    out
}

/// The palette's state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Palette {
    pub query: String,
    entries: Vec<Entry>,
    /// Which row is highlighted. `None` when nothing is, which is the state on
    /// open — the first entry is often a destructive routine and pre-selecting
    /// it means a stray Enter performs an edit nobody chose.
    selected: Option<usize>,
}

impl Palette {
    pub fn entries(&self) -> &[Entry] { &self.entries }
    pub fn selected(&self) -> Option<usize> { self.selected }

    pub fn set_entries(&mut self, e: Vec<Entry>) {
        // A selection that no longer exists is dropped rather than clamped:
        // clamping would silently move the highlight onto a different action
        // between keystrokes, and the user's next Enter would run it.
        self.selected = None;
        self.entries = e;
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.entries.is_empty() { self.selected = None; return; }
        let n = self.entries.len() as i32;
        self.selected = Some(match self.selected {
            None if delta > 0 => 0,
            None => (n - 1) as usize,
            Some(i) => (((i as i32 + delta) % n + n) % n) as usize,
        });
    }

    /// Take the highlighted entry, if there is one.
    ///
    /// Returns a reference for the host to act on; the palette performs nothing
    /// itself, because a routine's placeholders need context it does not have.
    pub fn chosen(&self) -> Option<&Entry> {
        self.entries.get(self.selected?)
    }

    pub fn clear(&mut self) {
        self.query.clear();
        self.entries.clear();
        self.selected = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{ClipId, Ticks};

    fn routine(name: &str, phrases: &[&str], uses: u32) -> Routine {
        Routine { name: name.into(),
                  phrases: phrases.iter().map(|s| s.to_string()).collect(),
                  steps: Vec::new(), uses }
    }

    fn hit(text: &str) -> Hit {
        Hit { clip: ClipId(1), at: Ticks(0), text: text.into(),
              speaker: None, confidence: 0.9 }
    }

    #[test]
    fn routines_rank_above_transcript_hits() {
        // They are destructive and few; hits are safe and many. A hundred
        // matches above three commands buries the commands, and a palette is
        // reached for precisely when the user does not want to hunt.
        let reps = vec![routine("Tighten", &["tighten this", "tighten"], 0)];
        let hits = vec![hit("tighten the shot"), hit("tighten again")];
        let e = entries(&reps, hits, "tighten");
        assert!(e[0].is_destructive());
        assert!(!e[1].is_destructive());
    }

    #[test]
    fn a_routine_ranks_by_its_closest_alias() {
        // People do not say the same words twice — "tighten this", "tighten it
        // up", "tighten" — so the best-matching phrase decides, not the first.
        let reps = vec![routine("Tighten", &["make it snappier", "tighten"], 0)];
        let e = entries(&reps, vec![], "tighten");
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn usage_breaks_ties_and_does_not_outrank_a_better_match() {
        // Otherwise the palette stops responding to what you typed and starts
        // second-guessing you.
        let reps = vec![routine("Rarely", &["tighten the edit"], 0),
                        routine("Often", &["tighten"], 99)];
        // Both match "tighten"; "tighten" is an exact hit and outranks the
        // longer phrase regardless of usage.
        let e = entries(&reps, vec![], "tighten");
        assert_eq!(e[0].label(), "Often");

        // With equal match quality, usage decides.
        let tied = vec![routine("Cold", &["trim"], 1), routine("Warm", &["trim"], 50)];
        let e2 = entries(&tied, vec![], "trim");
        assert_eq!(e2[0].label(), "Warm");
    }

    #[test]
    fn ranking_is_deterministic_when_everything_ties() {
        // A palette whose order changed between keystrokes would move the
        // highlight under the user's finger.
        let reps = vec![routine("Beta", &["x"], 5), routine("Alpha", &["x"], 5)];
        let e = entries(&reps, vec![], "x");
        assert_eq!(e[0].label(), "Alpha");
        assert_eq!(entries(&reps, vec![], "x")[0].label(), "Alpha");
    }

    #[test]
    fn an_empty_query_matches_nothing() {
        // Showing every routine for an empty box makes the first keystroke
        // reshuffle the whole list, and a palette that flashes is one people
        // close.
        let reps = vec![routine("Tighten", &["tighten"], 3)];
        assert!(entries(&reps, vec![hit("anything")], "").is_empty());
        assert!(entries(&reps, vec![], "   ").is_empty());
        assert_eq!(score("tighten", ""), None);
    }

    #[test]
    fn matching_is_prefix_then_substring_and_nothing_clever() {
        // A fuzzy matcher that scored "tighten" against "the lighting" would
        // put an edit above a navigation on a query meaning neither. In a
        // palette a surprising match is worse than a missing one.
        assert!(score("tighten", "tighten") > score("tighten this", "tighten"));
        assert!(score("tighten this", "tighten") > score("really tighten", "tighten"));
        assert_eq!(score("the lighting", "tighten"), None);
    }

    #[test]
    fn transcript_hits_keep_the_order_the_index_gave_them() {
        // TranscriptIndex::find already mapped them through the clips that
        // reference each source; their order encodes timeline position, which
        // is what the user is navigating by. Re-ranking by text would scramble
        // it.
        let hits = vec![hit("third"), hit("first"), hit("second")];
        let e = entries(&[], hits, "x");
        let labels: Vec<&str> = e.iter().map(|x| x.label()).collect();
        assert_eq!(labels, vec!["third", "first", "second"]);
    }

    #[test]
    fn nothing_is_selected_when_the_palette_opens() {
        // The first entry is often a destructive routine, and pre-selecting it
        // means a stray Enter performs an edit nobody chose.
        let mut p = Palette::default();
        p.set_entries(entries(&[routine("Delete all", &["delete"], 0)], vec![], "delete"));
        assert_eq!(p.selected(), None);
        assert!(p.chosen().is_none());
    }

    #[test]
    fn arrowing_down_starts_at_the_top_and_up_starts_at_the_bottom() {
        let mut p = Palette::default();
        p.set_entries(vec![Entry::Hit(hit("a")), Entry::Hit(hit("b"))]);
        p.move_selection(1);
        assert_eq!(p.selected(), Some(0));
        p.set_entries(vec![Entry::Hit(hit("a")), Entry::Hit(hit("b"))]);
        p.move_selection(-1);
        assert_eq!(p.selected(), Some(1));
    }

    #[test]
    fn the_selection_wraps() {
        let mut p = Palette::default();
        p.set_entries(vec![Entry::Hit(hit("a")), Entry::Hit(hit("b"))]);
        p.move_selection(1);
        p.move_selection(1);
        p.move_selection(1);
        assert_eq!(p.selected(), Some(0));
    }

    #[test]
    fn retyping_drops_the_selection_rather_than_clamping_it() {
        // Clamping would silently move the highlight onto a DIFFERENT action
        // between keystrokes, and the next Enter would run it.
        let mut p = Palette::default();
        p.set_entries(vec![Entry::Hit(hit("a")), Entry::Hit(hit("b")),
                           Entry::Hit(hit("c"))]);
        p.move_selection(1);
        p.move_selection(1);
        assert_eq!(p.selected(), Some(1));
        p.set_entries(vec![Entry::Hit(hit("z"))]);
        assert_eq!(p.selected(), None);
    }

    #[test]
    fn moving_within_an_empty_list_selects_nothing() {
        let mut p = Palette::default();
        p.move_selection(1);
        assert_eq!(p.selected(), None);
        assert!(p.chosen().is_none());
    }

    #[test]
    fn the_palette_performs_nothing_itself() {
        // A routine's steps carry placeholders the caller fills from context —
        // the selection, the playhead. Running "tighten this" with nothing
        // selected would no-op silently or guess at a target, and both are
        // worse than a refusal that says why.
        let mut p = Palette::default();
        p.set_entries(entries(&[routine("Tighten", &["tighten"], 0)], vec![], "tighten"));
        p.move_selection(1);
        match p.chosen() {
            Some(Entry::Routine { index, .. }) => assert_eq!(*index, 0),
            other => panic!("expected a routine reference, got {other:?}"),
        }
    }

    #[test]
    fn navigation_and_edits_are_distinguishable() {
        // A palette that presents them identically trains people to press Enter
        // without reading.
        assert!(Entry::Routine { index: 0, name: "x".into(), uses: 0 }.is_destructive());
        assert!(!Entry::Hit(hit("x")).is_destructive());
    }
}
