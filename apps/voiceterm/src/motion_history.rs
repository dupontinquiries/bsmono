//! v0.1.609: motion rollback — the pure position-history core
//! (TODO_motion_rollback option 1's testable piece). Tracks where the
//! cursor has been as (line, col) positions and walks backward through
//! that history on "rollback", generating the helix keystrokes to return
//! to a remembered position.
//!
//! What's pure + here: the PositionStack (push/pop/peek with coalescing
//! of same-spot revisits) and `keys_to_reach(target)` — turning a target
//! (line, col) into the helix keys to get there. What's build-gated and
//! NOT here: parsing helix's status line to LEARN the current (line,col)
//! each move (fragile, coupled to helix's status format — see the TODO).
//! This core can be built + tested ahead of that parsing.
//!
//! Coordinates are 1-based line, 0-based col, matching how helix reports
//! line:col in its status bar (line 1 is the first line).

/// A logical cursor position in the edited file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pos {
    pub line: u32, // 1-based
    pub col: u32,  // 0-based
}

impl Pos {
    pub fn new(line: u32, col: u32) -> Self {
        Pos { line, col }
    }
}

/// A bounded history of visited positions, newest on top. `record`
/// pushes a position unless it's the same line as the current top (a
/// horizontal-only wiggle on one line coalesces — we only remember the
/// last spot on a line, so rollback walks line-to-line, not keystroke-
/// to-keystroke, matching the user's "keep the backwards count" intent).
#[derive(Debug, Default)]
pub struct PositionStack {
    stack: Vec<Pos>,
    cap: usize,
}

impl PositionStack {
    /// `cap` bounds memory; 0 means a sensible default (200).
    pub fn new(cap: usize) -> Self {
        PositionStack { stack: Vec::new(), cap: if cap == 0 { 200 } else { cap } }
    }

    /// Record a newly-arrived-at position. Coalesces consecutive
    /// positions on the SAME line (keeps only the latest), so a burst of
    /// horizontal motion on one line is one history entry. An exact
    /// duplicate of the top is ignored.
    pub fn record(&mut self, pos: Pos) {
        if let Some(top) = self.stack.last() {
            if top.line == pos.line {
                // same line → update the col in place (coalesce)
                let last = self.stack.last_mut().unwrap();
                last.col = pos.col;
                return;
            }
        }
        self.stack.push(pos);
        if self.stack.len() > self.cap {
            // drop oldest (front) to stay bounded
            let overflow = self.stack.len() - self.cap;
            self.stack.drain(0..overflow);
        }
    }

    /// Pop the most recent position (the place to roll back TO). Returns
    /// None when history is empty.
    pub fn rollback(&mut self) -> Option<Pos> {
        self.stack.pop()
    }

    /// Peek the most recent position without removing it.
    pub fn peek(&self) -> Option<Pos> {
        self.stack.last().copied()
    }

    pub fn len(&self) -> usize {
        self.stack.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    pub fn clear(&mut self) {
        self.stack.clear();
    }
}

/// Generate the helix normal-mode keystrokes to move the cursor to
/// `target` (1-based line, 0-based col). Strategy:
///   - `<line>G` jumps to the line (helix: a count before G goes to that
///     line; `1G` is line 1).
///   - `gh` moves to line start (column 0), then `<col>l` moves right.
///     (Using gh + count-l is robust: it doesn't depend on where the
///     cursor was horizontally before the line jump.)
/// col 0 emits just the line jump + `gh` (no rightward motion).
///
/// Returns a keystroke string ready to feed to the PTY. This is the
/// "turn a remembered position into keys" half the TODO called out as
/// testable.
pub fn keys_to_reach(target: Pos) -> String {
    let mut out = String::new();
    // Jump to the line. Helix: <count>G → go to line <count>. Lines are
    // 1-based; clamp defensively so an out-of-contract line=0 emits a
    // valid "1G" rather than "0G" (which helix doesn't treat as a real
    // goto). A correct caller never passes 0, but keys_to_reach must not
    // emit invalid motions regardless.
    let line = target.line.max(1);
    out.push_str(&line.to_string());
    out.push('G');
    // Go to line start, then step right to the column.
    out.push_str("gh");
    if target.col > 0 {
        out.push_str(&target.col.to_string());
        out.push('l');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_rollback_lifo() {
        let mut s = PositionStack::new(0);
        s.record(Pos::new(10, 0));
        s.record(Pos::new(20, 5));
        s.record(Pos::new(30, 2));
        assert_eq!(s.rollback(), Some(Pos::new(30, 2)));
        assert_eq!(s.rollback(), Some(Pos::new(20, 5)));
        assert_eq!(s.rollback(), Some(Pos::new(10, 0)));
        assert_eq!(s.rollback(), None); // empty
    }

    #[test]
    fn same_line_coalesces() {
        let mut s = PositionStack::new(0);
        s.record(Pos::new(5, 0));
        s.record(Pos::new(5, 3)); // same line → coalesce, update col
        s.record(Pos::new(5, 8)); // same line again
        assert_eq!(s.len(), 1, "same-line moves should be one entry");
        assert_eq!(s.peek(), Some(Pos::new(5, 8)), "keeps latest col");
    }

    #[test]
    fn different_lines_stack() {
        let mut s = PositionStack::new(0);
        s.record(Pos::new(1, 0));
        s.record(Pos::new(2, 0));
        s.record(Pos::new(1, 0)); // back to line 1 — distinct entry (top was line 2)
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn exact_duplicate_top_is_coalesced() {
        let mut s = PositionStack::new(0);
        s.record(Pos::new(7, 4));
        s.record(Pos::new(7, 4)); // identical → same-line coalesce keeps 1
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn cap_bounds_history() {
        let mut s = PositionStack::new(3);
        for i in 1..=10u32 {
            s.record(Pos::new(i, 0)); // distinct lines
        }
        assert_eq!(s.len(), 3, "capped at 3");
        // newest retained
        assert_eq!(s.peek(), Some(Pos::new(10, 0)));
        // oldest-of-kept is line 8 (8,9,10)
        s.rollback(); s.rollback();
        assert_eq!(s.rollback(), Some(Pos::new(8, 0)));
    }

    #[test]
    fn rollback_empty_is_none() {
        let mut s = PositionStack::new(0);
        assert_eq!(s.rollback(), None);
        assert!(s.is_empty());
    }

    #[test]
    fn keys_to_reach_with_column() {
        // line 42, col 5 → "42Ggh5l"
        assert_eq!(keys_to_reach(Pos::new(42, 5)), "42Ggh5l");
    }

    #[test]
    fn keys_to_reach_col_zero_no_rightward() {
        // col 0 → just line jump + line start, no "0l"
        assert_eq!(keys_to_reach(Pos::new(7, 0)), "7Ggh");
    }

    #[test]
    fn keys_to_reach_line_one() {
        assert_eq!(keys_to_reach(Pos::new(1, 0)), "1Ggh");
    }

    #[test]
    fn keys_to_reach_clamps_invalid_line_zero() {
        // v0.1.622: line is 1-based; an out-of-contract 0 must still
        // produce VALID helix ("1G"), not "0G". Probe-found defensive gap.
        assert_eq!(keys_to_reach(Pos::new(0, 0)), "1Ggh");
        assert_eq!(keys_to_reach(Pos::new(0, 3)), "1Ggh3l");
    }

    #[test]
    fn round_trip_record_then_reach() {
        // simulate: visit several lines, roll back, get keys for the spot
        let mut s = PositionStack::new(0);
        s.record(Pos::new(3, 2));
        s.record(Pos::new(50, 10));
        let back = s.rollback().unwrap(); // 50,10
        assert_eq!(keys_to_reach(back), "50Ggh10l");
        let back2 = s.rollback().unwrap(); // 3,2
        assert_eq!(keys_to_reach(back2), "3Ggh2l");
    }
}
