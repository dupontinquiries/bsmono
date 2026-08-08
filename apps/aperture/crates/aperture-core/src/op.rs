//! op — an edit as a value.
//!
//! Two variants are enough for text; both carry the text they touch, so every
//! op knows its own inverse without consulting the buffer it was applied to.
//! That property is what makes the history a plain `Vec<Vec<Op>>` and keeps
//! the door open to studio-style collab (ops are the wire format) — see
//! apps/studio/README.md for why "edit as a value" beats command classes.
//!
//! Addresses are char indices *at the moment of application*. Within a group
//! ops are applied in order and inverted in reverse, so intra-group index
//! shifting is already accounted for.

use crate::buffer::Buffer;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Insert `text` so that its first char lands at char index `at`.
    Insert { at: usize, text: String },
    /// Delete `text` currently occupying `[at, at + text.chars().count())`.
    /// Carrying the text (not just the range) is what makes this invertible.
    Delete { at: usize, text: String },
}

impl Op {
    /// Build a Delete op by capturing the range's current content.
    pub fn delete_range(buf: &Buffer, start: usize, end: usize) -> Op {
        Op::Delete { at: start, text: buf.slice_to_string(start, end) }
    }

    pub fn invert(&self) -> Op {
        match self {
            Op::Insert { at, text } => Op::Delete { at: *at, text: text.clone() },
            Op::Delete { at, text } => Op::Insert { at: *at, text: text.clone() },
        }
    }

    pub fn apply(&self, buf: &mut Buffer) {
        match self {
            Op::Insert { at, text } => buf.insert(*at, text),
            Op::Delete { at, text } => {
                let n = text.chars().count();
                buf.remove(*at, *at + n);
            }
        }
    }

    /// Char index of the caret after this op — end of an insert, start of a delete.
    pub fn caret_after(&self) -> usize {
        match self {
            Op::Insert { at, text } => at + text.chars().count(),
            Op::Delete { at, .. } => *at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_then_inverse_is_identity() {
        let mut b = Buffer::from_str("abcdef");
        let ins = Op::Insert { at: 3, text: "XY".into() };
        ins.apply(&mut b);
        assert_eq!(b.to_string(), "abcXYdef");
        ins.invert().apply(&mut b);
        assert_eq!(b.to_string(), "abcdef");

        let del = Op::delete_range(&b, 1, 4);
        del.apply(&mut b);
        assert_eq!(b.to_string(), "aef");
        del.invert().apply(&mut b);
        assert_eq!(b.to_string(), "abcdef");
    }

    #[test]
    fn a_test_is_a_fold_of_ops() {
        // The studio property, verbatim in the text domain.
        let ops = [
            Op::Insert { at: 0, text: "fn main() {}".into() },
            Op::Insert { at: 11, text: "\n    println!()\n".into() },
        ];
        let mut b = Buffer::default();
        for op in &ops {
            op.apply(&mut b);
        }
        assert_eq!(b.to_string(), "fn main() {\n    println!()\n}");
    }
}
