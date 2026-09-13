//! helix — modal motions, as a DECISION over a keystroke rather than an
//! editor that owns state.
//!
//! Typewriter mode already changes what the camera does; this changes what
//! the keys do while it is on. The two travel together on purpose: a
//! centred, film-style caret is a reading-and-navigating posture, and
//! modal motion is what that posture wants. Turning one on without the
//! other would give you a camera for a mode you cannot drive.
//!
//! ## Why a table and not a match arm in the shell
//!
//! `resolve` maps (mode, key) to an [`Act`] and nothing else. It does not
//! touch a buffer, a clipboard, or a caret. That is what makes the whole
//! keymap testable without an editor, a window or a frame — and this
//! keymap has the kind of bugs that only show up in combination (does `d`
//! in Visual delete the selection or the char under the caret? does `i`
//! from Visual collapse the selection first?), which is exactly the class
//! a table makes visible and a scattered match hides.
//!
//! Helix, not vim, in one specific way that matters: SELECTION FIRST.
//! `w` extends a selection as it moves, and `d` deletes what is selected
//! rather than taking a motion argument. There is no `dw` — you press `w`
//! then `d`, and you can see what you are about to delete before you do
//! it. That is the whole reason to prefer this model, so operators here
//! never take a motion.

/// Which modal mode the editor is in. `Insert` is ordinary typing — every
/// key falls through to the buffer — so it is the mode the shell uses when
/// typewriter mode is OFF, and no key table applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Motions move the caret and collapse the selection.
    #[default]
    Normal,
    /// Motions EXTEND the selection instead of collapsing it.
    Visual,
    /// Keys are text.
    Insert,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NOR",
            Mode::Visual => "VIS",
            Mode::Insert => "INS",
        }
    }
}

/// Which way a motion goes, in the units the editor already understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordNext,
    WordPrev,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
}

/// What a keystroke asked for. The shell performs these; nothing here does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Move (Normal) or extend (Visual) — the caller reads the mode.
    Move(Motion),
    /// Delete the selection, or the character under the caret when the
    /// selection is empty.
    Delete,
    /// Copy the selection.
    Yank,
    /// Insert the last yank at the caret.
    Paste,
    /// Enter Insert mode at the given edge of the current selection.
    /// `Before` is `i`, `After` is `a` — and from a SELECTION they mean
    /// its start and end, which is why they carry an edge rather than
    /// being one variant plus a caret nudge.
    Insert { at: Edge },
    /// `Shift-I` / `Shift-A`: jump to this line's start/end, THEN enter
    /// Insert — one act, not a `Move` followed by a separate `Insert`.
    ///
    /// A first version of this returned `Act::Move(Motion::LineStart)` for
    /// `I`, on the reasoning that "go to line start" was the whole of what
    /// the key does and Insert would follow from context. It does not:
    /// `next_mode` only enters Insert on `Act::Insert`/`Act::OpenLine`, and
    /// falls through to `_ => mode` for everything else including `Move` —
    /// so `Shift-I` moved the caret and left the editor in Normal mode,
    /// silently. The bug is exactly the trap this module's own doc warns
    /// about: "does `d` in Visual delete the selection or the char under
    /// the caret" is the class of bug a table makes visible, and this was
    /// the same class the other way — an act that LOOKED complete (it did
    /// move the caret) while quietly missing the second half of what the
    /// key means.
    InsertAtLineEdge { end: bool },
    /// Open a new line and enter Insert: `o` below, `O` above.
    OpenLine { above: bool },
    /// `x`: select the current line (extending onto the next line if a
    /// line is already selected, as Helix's `x` does on repeat). Distinct
    /// from `Delete` — Helix's `x` does not delete anything, it is a
    /// selection primitive you then operate on with `d`/`y`/`c`. Binding
    /// it to `Delete` (as this table used to) collapses two different
    /// verbs onto one key: the FIRST press of real Helix `x` never
    /// deletes.
    SelectLine,
    EnterVisual,
    /// Leave Visual (or leave Insert) for Normal.
    EnterNormal,
    /// Hand off to the shell's route/goto palette — `f` is bound here
    /// rather than to a find-char motion, per the ask: the useful "go
    /// somewhere" verb in this editor is already Ctrl+G, and giving `f`
    /// a second, weaker meaning would be the worse of two bindings.
    Route,
    /// `/`: open the in-buffer incremental search bar (`bufsearch`).
    Search,
    /// Open the command palette. Bound to Space in Normal mode, alongside
    /// (not instead of) Ctrl+P — Helix's own leader key is Space, and a
    /// modal editor without it reads as an incomplete port to anyone
    /// coming from Helix, which is the whole point of this table.
    /// Insert mode's Space is unaffected: `resolve` returns `Act::Pass`
    /// for the entire mode before this arm is ever reached, so a space
    /// bar keystroke while typing is never at risk of being reinterpreted.
    CommandPalette,
    /// Cycle the completion debounce (250 -> 750 -> 1250 -> 250 ms).
    CycleCompletionSpeed,
    /// Nothing is bound; the shell may treat the key normally.
    Pass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Before,
    After,
}

/// The debounce rungs `c` cycles through, in milliseconds.
///
/// Not a slider: three named rungs a person can feel the difference
/// between. 250 is "as I type" and costs the most battery because nearly
/// every pause triggers a request; 1250 only fires when you genuinely
/// stop, which on a laptop away from power is the difference between a
/// warm machine and a cool one. The middle rung exists because the jump
/// from one to the other is too large to be the only choice.
pub const DEBOUNCE_RUNGS_MS: [u64; 3] = [250, 750, 1250];

/// Next rung, wrapping. An unrecognised current value snaps to the first
/// rung rather than being preserved — the cycle must reach a known state
/// on one press however it was configured.
pub fn next_debounce(current_ms: u64) -> u64 {
    match DEBOUNCE_RUNGS_MS.iter().position(|r| *r == current_ms) {
        Some(i) => DEBOUNCE_RUNGS_MS[(i + 1) % DEBOUNCE_RUNGS_MS.len()],
        None => DEBOUNCE_RUNGS_MS[0],
    }
}

/// The whole keymap. `shift` is separate because `A`/`I`/`O` are
/// different acts from `a`/`i`/`o`, not modified ones.
pub fn resolve(mode: Mode, ch: char, shift: bool) -> Act {
    if mode == Mode::Insert {
        return Act::Pass; // typing is typing
    }
    match (ch, shift) {
        // hjkl. Kept because the whole point is not moving the hand to the
        // arrow keys; arrows still work, they just are not the reason.
        ('h', false) => Act::Move(Motion::Left),
        ('j', false) => Act::Move(Motion::Down),
        ('k', false) => Act::Move(Motion::Up),
        ('l', false) => Act::Move(Motion::Right),

        ('w', false) => Act::Move(Motion::WordNext),
        ('b', false) => Act::Move(Motion::WordPrev),

        // Line and document ends, so a motion vocabulary that stops at the
        // word is not the whole vocabulary.
        ('0', false) => Act::Move(Motion::LineStart),
        ('$', _) => Act::Move(Motion::LineEnd),
        ('g', false) => Act::Move(Motion::DocStart),
        ('g', true) => Act::Move(Motion::DocEnd),

        ('x', false) => Act::SelectLine,
        ('d', false) => Act::Delete,
        ('y', false) => Act::Yank,
        ('p', false) => Act::Paste,

        ('i', false) => Act::Insert { at: Edge::Before },
        ('a', false) => Act::Insert { at: Edge::After },
        // I: to line start, then Insert. A: to line end, then Insert.
        // See `Act::InsertAtLineEdge`'s doc for why this is one act and
        // not `Act::Move` followed by a separate insert.
        ('i', true) => Act::InsertAtLineEdge { end: false },
        ('a', true) => Act::InsertAtLineEdge { end: true },
        ('o', false) => Act::OpenLine { above: false },
        ('o', true) => Act::OpenLine { above: true },

        ('v', false) => Act::EnterVisual,

        ('f', false) => Act::Route,
        ('/', _) => Act::Search,
        (' ', false) => Act::CommandPalette,
        ('c', false) => Act::CycleCompletionSpeed,

        _ => Act::Pass,
    }
}

/// The mode a key leaves the editor in. Separate from [`resolve`] because
/// the same act ends in different modes depending on where it started:
/// `d` from Visual returns to Normal (the selection is gone, so staying in
/// Visual would leave you extending from nothing), while `d` from Normal
/// stays in Normal.
pub fn next_mode(mode: Mode, act: &Act) -> Mode {
    match act {
        Act::EnterVisual => Mode::Visual,
        Act::EnterNormal => Mode::Normal,
        Act::Insert { .. } | Act::OpenLine { .. } | Act::InsertAtLineEdge { .. } => Mode::Insert,
        // Selecting a line is a Visual-shaped act even when pressed from
        // Normal — Helix's `x` puts you in a state where the next
        // keystroke (d/y/c, or another x to extend) operates on a
        // selection, which IS what Visual mode means here.
        Act::SelectLine => Mode::Visual,
        // An operator consumes the selection, so Visual has nothing left
        // to extend.
        Act::Delete | Act::Yank | Act::Paste => Mode::Normal,
        _ => mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_mode_binds_nothing_so_typing_is_typing() {
        // The property that makes modal editing survivable: in Insert,
        // every letter must reach the buffer. A single stolen key here
        // (a `d` that deletes while you are writing "add") is the failure
        // that makes people turn the whole feature off.
        //
        // Space and `/` are in this list for the same reason and are the
        // two keys most likely to break it if this table ever grows a
        // second special case for Insert mode: Space is now the command
        // palette leader and `/` is now search, both bound in Normal — if
        // either leaked into Insert, every sentence containing a space or
        // a slash (which is nearly every sentence, and every URL and path)
        // would stop being typeable text.
        for ch in ['h', 'j', 'k', 'l', 'w', 'b', 'x', 'y', 'p', 'd', 'i', 'a', 'v', 'f', 'c', ' ', '/'] {
            assert_eq!(resolve(Mode::Insert, ch, false), Act::Pass, "{ch} was stolen in Insert");
        }
    }

    #[test]
    fn hjkl_and_word_motions_resolve_in_both_modal_modes() {
        // Same keys, both modes — the DIFFERENCE is what the caller does
        // with them (move vs extend), not which keys exist. Two tables
        // would drift.
        for mode in [Mode::Normal, Mode::Visual] {
            assert_eq!(resolve(mode, 'h', false), Act::Move(Motion::Left));
            assert_eq!(resolve(mode, 'j', false), Act::Move(Motion::Down));
            assert_eq!(resolve(mode, 'k', false), Act::Move(Motion::Up));
            assert_eq!(resolve(mode, 'l', false), Act::Move(Motion::Right));
            assert_eq!(resolve(mode, 'w', false), Act::Move(Motion::WordNext));
            assert_eq!(resolve(mode, 'b', false), Act::Move(Motion::WordPrev));
        }
    }

    #[test]
    fn operators_take_no_motion_which_is_the_point_of_helix() {
        // There is no `dw`. You select with `w`, SEE it, then `d`. If an
        // operator ever started consuming a following motion, the visible
        // selection would stop being the thing that gets deleted, which is
        // the whole reason to prefer this model over vim's.
        assert_eq!(resolve(Mode::Normal, 'd', false), Act::Delete);
        assert_eq!(resolve(Mode::Normal, 'w', false), Act::Move(Motion::WordNext));
        assert_eq!(resolve(Mode::Visual, 'd', false), Act::Delete);
    }

    #[test]
    fn an_operator_returns_to_normal_from_visual() {
        // Staying in Visual after `d` would leave the caret extending a
        // selection that no longer exists.
        assert_eq!(next_mode(Mode::Visual, &Act::Delete), Mode::Normal);
        assert_eq!(next_mode(Mode::Visual, &Act::Yank), Mode::Normal);
        // But a motion in Visual stays in Visual, or the selection could
        // never grow past one step.
        assert_eq!(next_mode(Mode::Visual, &Act::Move(Motion::WordNext)), Mode::Visual);
    }

    #[test]
    fn i_and_a_carry_the_edge_they_insert_at() {
        // From a SELECTION these mean its start and its end, so the edge
        // is part of the act rather than a caret nudge the caller has to
        // remember to apply in the right direction.
        assert_eq!(resolve(Mode::Normal, 'i', false), Act::Insert { at: Edge::Before });
        assert_eq!(resolve(Mode::Normal, 'a', false), Act::Insert { at: Edge::After });
        assert_eq!(next_mode(Mode::Visual, &Act::Insert { at: Edge::Before }), Mode::Insert);
    }

    #[test]
    fn shift_i_and_shift_a_actually_enter_insert_mode() {
        // The bug this fixes: an earlier version of `resolve` returned
        // `Act::Move(Motion::LineStart)` for Shift-I, which moved the
        // caret correctly and then, because `next_mode` does not enter
        // Insert on a bare `Move`, left the editor in Normal mode. Typing
        // the next character would have run it through the modal table
        // instead of inserting it — silently, since the caret WAS in the
        // right place and nothing looked wrong until you typed.
        assert_eq!(resolve(Mode::Normal, 'i', true), Act::InsertAtLineEdge { end: false });
        assert_eq!(resolve(Mode::Normal, 'a', true), Act::InsertAtLineEdge { end: true });
        assert_eq!(
            next_mode(Mode::Normal, &Act::InsertAtLineEdge { end: false }),
            Mode::Insert,
            "Shift-I must leave Insert mode active, not just move the caret"
        );
        assert_eq!(next_mode(Mode::Normal, &Act::InsertAtLineEdge { end: true }), Mode::Insert);
        // And from Visual, same as plain `i`/`a` — jumping to a line edge
        // while a selection is active must still end in Insert, not leave
        // a stale Visual selection behind.
        assert_eq!(next_mode(Mode::Visual, &Act::InsertAtLineEdge { end: false }), Mode::Insert);
    }

    #[test]
    fn x_selects_the_line_rather_than_deleting_it() {
        // Helix's `x` is a selection primitive, not an operator — the
        // FIRST press never deletes anything. This table used to bind `x`
        // to `Act::Delete`, identically to `d`, which collapsed two
        // distinct verbs onto one key and meant there was no way to
        // select-then-decide (delete? yank? insert before?) the way real
        // Helix's line selection works.
        assert_eq!(resolve(Mode::Normal, 'x', false), Act::SelectLine);
        assert_ne!(
            resolve(Mode::Normal, 'x', false),
            Act::Delete,
            "x must not be a second spelling of d"
        );
        // Selecting a line produces something to operate on, so it must
        // land in the mode where a following d/y/c has a selection to
        // consume — otherwise `x` then `d` would delete a bare caret
        // position instead of the whole line `x` just selected.
        assert_eq!(next_mode(Mode::Normal, &Act::SelectLine), Mode::Visual);
    }

    #[test]
    fn slash_opens_search_from_either_modal_mode() {
        for mode in [Mode::Normal, Mode::Visual] {
            assert_eq!(resolve(mode, '/', false), Act::Search);
        }
        // Search does not itself change the modal mode — closing the
        // search bar (Escape) is what returns to Normal, same as the
        // route palette (`f`) leaves the mode alone until it is dismissed.
        assert_eq!(next_mode(Mode::Normal, &Act::Search), Mode::Normal);
    }

    #[test]
    fn space_opens_the_command_palette_only_from_normal() {
        assert_eq!(resolve(Mode::Normal, ' ', false), Act::CommandPalette);
        // Shifted space (a keyboard oddity, but resolve must not treat it
        // as a different key by accident) falls through to Pass rather
        // than silently also opening the palette.
        assert_eq!(resolve(Mode::Normal, ' ', true), Act::Pass);
    }

    #[test]
    fn f_routes_rather_than_finding_a_character() {
        // Deliberate divergence from helix: this editor's useful "go
        // somewhere" verb is the route palette, and giving `f` a second
        // weaker meaning would waste the best key on the worse binding.
        assert_eq!(resolve(Mode::Normal, 'f', false), Act::Route);
        assert_eq!(next_mode(Mode::Normal, &Act::Route), Mode::Normal, "routing is not a mode");
    }

    #[test]
    fn g_and_shift_g_reach_both_ends_of_the_document() {
        assert_eq!(resolve(Mode::Normal, 'g', false), Act::Move(Motion::DocStart));
        assert_eq!(resolve(Mode::Normal, 'g', true), Act::Move(Motion::DocEnd));
    }

    #[test]
    fn the_debounce_cycle_visits_every_rung_and_returns() {
        // Three presses from any rung must come home, or the control is a
        // one-way trip and the fastest setting becomes unreachable.
        let mut ms = DEBOUNCE_RUNGS_MS[0];
        let mut seen = vec![ms];
        for _ in 0..DEBOUNCE_RUNGS_MS.len() {
            ms = next_debounce(ms);
            seen.push(ms);
        }
        assert_eq!(seen.first(), seen.last(), "the cycle does not return: {seen:?}");
        for r in DEBOUNCE_RUNGS_MS {
            assert!(seen.contains(&r), "rung {r} is unreachable: {seen:?}");
        }
    }

    #[test]
    fn an_unknown_debounce_snaps_to_a_known_rung() {
        // A value from an older config, or hand-edited: one press must
        // land somewhere the cycle can continue from, not preserve a
        // number outside the rungs forever.
        let next = next_debounce(999);
        assert!(DEBOUNCE_RUNGS_MS.contains(&next), "snapped to {next}");
    }

    #[test]
    fn unbound_keys_pass_through_rather_than_being_swallowed() {
        // Anything not in the table must reach the shell, or adding a
        // binding elsewhere silently stops working the moment modal mode
        // is on.
        for ch in ['z', 'q', '7', '@'] {
            assert_eq!(resolve(Mode::Normal, ch, false), Act::Pass, "{ch}");
        }
    }
}
