//! keys — Helix-style modal editing for a timeline.
//!
//! ## Why Helix's grammar, specifically
//!
//! Helix is selection-first: you select a thing, then act on it (`w` then `d`),
//! where vim is verb-first (`dw`). For a text buffer that is a preference. For a
//! **timeline** it is simply how editing already works — you click a clip and
//! then do something to it, and every NLE on earth has that shape. Helix's model
//! therefore maps on with almost no distortion, and brings two things a
//! timeline badly wants and mouse-driven editors rarely give you:
//!
//! * **Multiple selections as a first-class idea.** `C` adds a cursor, so "trim
//!   the head of these nine clips" is one operation rather than nine drags.
//!   kdenlive and Audacity both make this awkward; Helix makes it the default
//!   way of thinking.
//! * **Motions that mean something musically.** In text `w` is a word; on a
//!   timeline the equivalent unit is the **edit point** — a clip boundary. So
//!   `w`/`b` step cut to cut, which is exactly how an editor navigates a rough
//!   assembly, and `e` goes to the end of the current clip.
//!
//! ## What is deliberately NOT copied
//!
//! Helix's regex selection (`s`), its tree-sitter objects, and `%` for "whole
//! file" have no timeline meaning, and inventing one would produce keys nobody
//! can guess. `s` is **split** here — the one place this departs from Helix, and
//! it departs toward what every video editor already binds `s` to. A key that
//! surprises both audiences is worse than one that only surprises half.
//!
//! Insert mode exists but means "typing into a field" (renaming a track, typing
//! a timecode), not inserting media. The escape hatch is the same: `Esc`.

use studio_core::{ClipId, Ticks};

/// Editing mode. `Select` is Helix's visual/extend mode: motions grow the
/// selection instead of replacing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Normal,
    Select,
    /// Text entry — a rename field, a timecode box. Every key is literal.
    Insert,
}

/// A key press, reduced to what the map cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub ch: char,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Key {
    pub fn c(ch: char) -> Key { Key { ch, shift: false, ctrl: false, alt: false } }
    pub fn shift(ch: char) -> Key { Key { ch, shift: true, ctrl: false, alt: false } }
    pub fn ctrl(ch: char) -> Key { Key { ch, shift: false, ctrl: true, alt: false } }
    pub fn alt(ch: char) -> Key { Key { ch, shift: false, ctrl: false, alt: true } }
}

/// What a resolved binding asks the editor to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    // ── motion (Helix `w`/`b`/`e`, reinterpreted as edit points) ────────────
    /// Next clip boundary. `count` applies, so `3w` skips three cuts.
    NextEdit { count: u32, extend: bool },
    PrevEdit { count: u32, extend: bool },
    /// End of the clip under the playhead.
    ClipEnd { extend: bool },
    /// `g g` / `g e` — start and end of the timeline.
    GotoStart,
    GotoEnd,
    /// `g c` — back to the playhead, the "where was I" key.
    GotoPlayhead,

    // ── selection ───────────────────────────────────────────────────────────
    /// `x` — select the whole clip under the playhead; repeats extend to the
    /// next one, as `x` extends line-wise in Helix.
    SelectClip { extend: bool },
    /// `%` — every clip on the timeline.
    SelectAll,
    /// `;` — collapse to a single selection at the playhead.
    CollapseSelection,
    /// `C` — add the clip below/above to the selection: multi-clip editing.
    AddSelectionBelow,
    AddSelectionAbove,

    // ── edits ───────────────────────────────────────────────────────────────
    Delete,
    Yank,
    Paste,
    /// `s` — split at the playhead. The one deliberate departure from Helix.
    Split,
    /// `alt-;` — flip the trim handle a drag would grab.
    FlipTrimSide,
    SetGain { delta: f32 },
    ToggleMute,
    ToggleLock,

    // ── history ─────────────────────────────────────────────────────────────
    Undo,
    Redo,

    // ── transport (no Helix equivalent; these are the editor's own) ─────────
    PlayPause,
    ToggleRecord,
    ToggleLoop,

    // ── modes and menus ─────────────────────────────────────────────────────
    EnterMode(Mode),
    /// `space` — the leader menu, as in Helix.
    LeaderMenu,
    /// `:` — the command palette.
    Command,
    /// A literal character, in Insert mode.
    Insert(char),
}

/// What feeding a key produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// A complete binding.
    Act(Action),
    /// A prefix so far (`g`, or a count being typed) — the caller should show
    /// it, the way Helix's status line does.
    Pending(String),
    /// Nothing bound. The pending state is cleared, because a half-typed
    /// sequence that silently survives a stray key is how a "haunted" editor
    /// feels.
    None,
}

/// The modal state machine.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    pub mode: Mode,
    /// Digits typed so far (Helix counts: `3w`).
    count: u32,
    /// A pending prefix such as `g`.
    prefix: Option<char>,
}

impl Keymap {
    pub fn new() -> Keymap { Keymap::default() }
    pub fn mode(&self) -> Mode { self.mode }
    /// The prefix/count to show in the status line.
    pub fn pending(&self) -> String {
        let mut s = String::new();
        if self.count > 0 { s.push_str(&self.count.to_string()); }
        if let Some(p) = self.prefix { s.push(p); }
        s
    }
    fn take_count(&mut self) -> u32 {
        let c = self.count.max(1);
        self.count = 0;
        c
    }
    fn clear(&mut self) { self.count = 0; self.prefix = None; }

    pub fn feed(&mut self, k: Key) -> Resolution {
        // Insert mode is literal apart from the escape hatch — otherwise typing
        // a track name called "wd" would delete something.
        if self.mode == Mode::Insert {
            if k.ch == '\u{1b}' {
                self.mode = Mode::Normal;
                return Resolution::Act(Action::EnterMode(Mode::Normal));
            }
            return Resolution::Act(Action::Insert(k.ch));
        }

        if k.ch == '\u{1b}' {
            // Esc leaves Select mode, and otherwise cancels whatever was pending
            // — one key that always means "back out", as in Helix.
            let was = self.mode;
            self.clear();
            self.mode = Mode::Normal;
            return Resolution::Act(Action::EnterMode(if was == Mode::Select { Mode::Normal } else { Mode::Normal }));
        }

        // a pending `g` (goto) — the only two-key prefix worth having here
        if self.prefix == Some('g') {
            self.prefix = None;
            self.count = 0;
            return match k.ch {
                'g' => Resolution::Act(Action::GotoStart),
                'e' => Resolution::Act(Action::GotoEnd),
                'c' => Resolution::Act(Action::GotoPlayhead),
                _ => Resolution::None,
            };
        }

        // counts, as in Helix: digits accumulate, but a leading 0 is not a count
        if k.ch.is_ascii_digit() && !(k.ch == '0' && self.count == 0) {
            self.count = self.count.saturating_mul(10) + k.ch.to_digit(10).unwrap();
            return Resolution::Pending(self.pending());
        }

        let extend = self.mode == Mode::Select;
        let act = match (k.ch, k.shift, k.ctrl, k.alt) {
            ('g', false, false, false) => { self.prefix = Some('g'); return Resolution::Pending(self.pending()); }

            // motions over edit points
            ('w', false, false, false) => Action::NextEdit { count: self.take_count(), extend },
            ('b', false, false, false) => Action::PrevEdit { count: self.take_count(), extend },
            ('e', false, false, false) => Action::ClipEnd { extend },

            // selection
            ('x', false, false, false) => Action::SelectClip { extend },
            ('%', _, false, false) => Action::SelectAll,
            (';', false, false, false) => Action::CollapseSelection,
            (';', false, false, true) => Action::FlipTrimSide,
            ('C', _, false, false) | ('c', true, false, false) => Action::AddSelectionBelow,
            ('C', _, false, true) | ('c', true, false, true) => Action::AddSelectionAbove,

            // edits
            ('d', false, false, false) => Action::Delete,
            ('y', false, false, false) => Action::Yank,
            ('p', false, false, false) => Action::Paste,
            ('s', false, false, false) => Action::Split,
            ('m', false, false, false) => Action::ToggleMute,
            ('l', false, false, false) => Action::ToggleLock,
            ('=', _, false, false) => Action::SetGain { delta: 0.05 },
            ('-', _, false, false) => Action::SetGain { delta: -0.05 },

            // history — Helix's own bindings, and they already match our History
            ('u', false, false, false) => Action::Undo,
            ('U', _, false, false) | ('u', true, false, false) => Action::Redo,

            // transport
            (' ', false, false, false) => Action::PlayPause,
            ('r', false, false, false) => Action::ToggleRecord,
            ('L', _, false, false) | ('l', true, false, false) => Action::ToggleLoop,

            // modes and menus
            ('v', false, false, false) => {
                self.mode = if self.mode == Mode::Select { Mode::Normal } else { Mode::Select };
                Action::EnterMode(self.mode)
            }
            ('i', false, false, false) => { self.mode = Mode::Insert; Action::EnterMode(Mode::Insert) }
            (' ', true, false, false) => Action::LeaderMenu,
            (':', _, false, false) => Action::Command,
            _ => { self.clear(); return Resolution::None; }
        };
        self.count = 0;
        Resolution::Act(act)
    }
}

/// Where a motion lands, given the edit points on the current track.
///
/// Kept separate from the keymap so it is testable without any UI, and because
/// "what is the next cut" is a timeline question, not a keyboard one.
pub fn next_edit(edits: &[Ticks], from: Ticks, count: u32, forward: bool) -> Ticks {
    let mut at = from;
    for _ in 0..count.max(1) {
        let next = if forward {
            edits.iter().filter(|e| **e > at).min()
        } else {
            edits.iter().filter(|e| **e < at).max()
        };
        match next {
            Some(t) => at = *t,
            // Stop at the ends rather than wrapping: an editor that jumps from
            // the last cut back to the head on one extra keypress loses you.
            None => break,
        }
    }
    at
}

/// Every clip boundary on a track, sorted — the "words" of a timeline.
pub fn edit_points(project: &studio_core::Project, track: studio_core::TrackId) -> Vec<Ticks> {
    let mut v: Vec<Ticks> = project
        .track_clips(track)
        .iter()
        .flat_map(|c| [c.pos, c.end()])
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Clips touched by a selection span — what an action applies to.
pub fn clips_in_span(
    project: &studio_core::Project,
    track: studio_core::TrackId,
    from: Ticks,
    to: Ticks,
) -> Vec<ClipId> {
    let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
    project
        .track_clips(track)
        .iter()
        .filter(|c| c.pos < hi && c.end() > lo)
        .map(|c| c.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Clip, Op, Project, Source, SourceId, Track, TrackId, TrackKind};

    fn km() -> Keymap { Keymap::new() }
    fn act(k: &mut Keymap, ch: char) -> Resolution { k.feed(Key::c(ch)) }

    #[test]
    fn counts_accumulate_and_apply_to_the_motion() {
        // Helix's `3w`. The count must reach the action, and must reset after —
        // a sticky count is the modal-editor bug everyone has met.
        let mut k = km();
        assert_eq!(act(&mut k, '3'), Resolution::Pending("3".into()));
        assert_eq!(act(&mut k, 'w'), Resolution::Act(Action::NextEdit { count: 3, extend: false }));
        assert_eq!(act(&mut k, 'w'), Resolution::Act(Action::NextEdit { count: 1, extend: false }),
                   "the count does not stick to the next motion");
        // multi-digit
        act(&mut k, '1'); act(&mut k, '2');
        assert_eq!(act(&mut k, 'b'), Resolution::Act(Action::PrevEdit { count: 12, extend: false }));
        // a leading zero is not a count (it would be a motion in Helix)
        let mut k = km();
        assert_eq!(act(&mut k, '0'), Resolution::None);
    }

    #[test]
    fn goto_is_a_two_key_sequence_that_shows_itself_and_can_be_abandoned() {
        let mut k = km();
        assert_eq!(act(&mut k, 'g'), Resolution::Pending("g".into()));
        assert_eq!(act(&mut k, 'g'), Resolution::Act(Action::GotoStart));
        act(&mut k, 'g');
        assert_eq!(act(&mut k, 'e'), Resolution::Act(Action::GotoEnd));
        act(&mut k, 'g');
        assert_eq!(act(&mut k, 'c'), Resolution::Act(Action::GotoPlayhead));
        // an unbound second key clears rather than lingering — a half-typed
        // sequence that survives a stray press is how an editor feels haunted
        act(&mut k, 'g');
        assert_eq!(act(&mut k, 'z'), Resolution::None);
        assert_eq!(k.pending(), "", "nothing left pending");
        assert_eq!(act(&mut k, 'd'), Resolution::Act(Action::Delete), "and the next key works normally");
    }

    #[test]
    fn select_mode_makes_motions_extend() {
        let mut k = km();
        assert_eq!(act(&mut k, 'w'), Resolution::Act(Action::NextEdit { count: 1, extend: false }));
        assert_eq!(act(&mut k, 'v'), Resolution::Act(Action::EnterMode(Mode::Select)));
        assert_eq!(k.mode(), Mode::Select);
        assert_eq!(act(&mut k, 'w'), Resolution::Act(Action::NextEdit { count: 1, extend: true }),
                   "in select mode a motion grows the selection");
        assert_eq!(act(&mut k, 'x'), Resolution::Act(Action::SelectClip { extend: true }));
        // v toggles back
        assert_eq!(act(&mut k, 'v'), Resolution::Act(Action::EnterMode(Mode::Normal)));
        assert_eq!(k.mode(), Mode::Normal);
    }

    #[test]
    fn insert_mode_is_literal_so_typing_a_name_cannot_edit_the_timeline() {
        // The trap: a track named "wd" must not step a cut and delete a clip.
        let mut k = km();
        assert_eq!(act(&mut k, 'i'), Resolution::Act(Action::EnterMode(Mode::Insert)));
        assert_eq!(act(&mut k, 'w'), Resolution::Act(Action::Insert('w')));
        assert_eq!(act(&mut k, 'd'), Resolution::Act(Action::Insert('d')));
        assert_eq!(act(&mut k, '3'), Resolution::Act(Action::Insert('3')));
        // Esc is the one escape hatch, the same as everywhere else
        assert_eq!(k.feed(Key::c('\u{1b}')), Resolution::Act(Action::EnterMode(Mode::Normal)));
        assert_eq!(act(&mut k, 'd'), Resolution::Act(Action::Delete));
    }

    #[test]
    fn esc_always_backs_out_of_whatever_is_pending() {
        let mut k = km();
        act(&mut k, '4');
        act(&mut k, 'g');
        assert!(!k.pending().is_empty());
        k.feed(Key::c('\u{1b}'));
        assert_eq!(k.pending(), "", "Esc cancels a count and a prefix together");
        assert_eq!(k.mode(), Mode::Normal);
    }

    #[test]
    fn multiple_selections_are_reachable_the_way_helix_makes_them() {
        // The reason for borrowing this grammar at all: "trim these nine clips"
        // as one operation instead of nine drags.
        let mut k = km();
        assert_eq!(k.feed(Key::shift('c')), Resolution::Act(Action::AddSelectionBelow));
        assert_eq!(k.feed(Key { ch: 'c', shift: true, ctrl: false, alt: true }),
                   Resolution::Act(Action::AddSelectionAbove));
        assert_eq!(act(&mut k, ';'), Resolution::Act(Action::CollapseSelection));
        assert_eq!(act(&mut k, '%'), Resolution::Act(Action::SelectAll));
    }

    #[test]
    fn the_editing_and_history_keys_are_the_ones_a_helix_user_already_knows() {
        let mut k = km();
        for (ch, want) in [('d', Action::Delete), ('y', Action::Yank), ('p', Action::Paste),
                           ('u', Action::Undo)] {
            assert_eq!(act(&mut k, ch), Resolution::Act(want));
        }
        assert_eq!(k.feed(Key::shift('u')), Resolution::Act(Action::Redo));
        // `s` is the deliberate departure: split, as every video editor binds it
        assert_eq!(act(&mut k, 's'), Resolution::Act(Action::Split));
    }

    #[test]
    fn edit_points_are_the_words_of_a_timeline() {
        let mut p = Project { name: "t".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video,
            name: "V1".into(), muted: false, locked: false }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "a".into(),
            len: Ticks::from_seconds(100.0), sample_rate: 48_000, channels: 2, has_video: true } }).unwrap();
        for (i, at) in [0.0f64, 5.0, 12.0].iter().enumerate() {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(i as u64), source: SourceId(1),
                track: TrackId(1), pos: Ticks::from_seconds(*at), src_in: Ticks::ZERO,
                len: Ticks::from_seconds(3.0), gain: 1.0, effects: Vec::new() } }).unwrap();
        }
        let e = edit_points(&p, TrackId(1));
        assert_eq!(e, vec![Ticks::ZERO, Ticks::from_seconds(3.0), Ticks::from_seconds(5.0),
                           Ticks::from_seconds(8.0), Ticks::from_seconds(12.0), Ticks::from_seconds(15.0)],
                   "every boundary, sorted and de-duplicated");

        // `w` steps cut to cut; `3w` skips three
        let from = Ticks::from_seconds(0.0);
        assert_eq!(next_edit(&e, from, 1, true), Ticks::from_seconds(3.0));
        assert_eq!(next_edit(&e, from, 3, true), Ticks::from_seconds(8.0));
        assert_eq!(next_edit(&e, Ticks::from_seconds(8.0), 1, false), Ticks::from_seconds(5.0));
        // and it stops at the ends instead of wrapping — an editor that jumps
        // from the last cut back to the head on one extra press loses you
        assert_eq!(next_edit(&e, from, 99, true), Ticks::from_seconds(15.0));
        assert_eq!(next_edit(&e, from, 5, false), from);

        // a selection spanning the first two clips reports both
        let sel = clips_in_span(&p, TrackId(1), Ticks::from_seconds(1.0), Ticks::from_seconds(6.0));
        assert_eq!(sel, vec![ClipId(0), ClipId(1)]);
        // and one in a gap reports none
        assert!(clips_in_span(&p, TrackId(1), Ticks::from_seconds(3.5),
                              Ticks::from_seconds(4.5)).is_empty());
    }
}
