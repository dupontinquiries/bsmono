//! v0.1.719: modeless count-prefix accumulator — the state machine behind
//! vim-style `{N}<cmd>` input (e.g. `3b` = cycle 3 tabs forward, `2B` back 2).
//! The QML key handler is a thin shell over this; all the tricky decisions
//! (accumulate a digit, cap it, when a key fires a command with what count,
//! when to reset, when to CONSUME the key vs let it pass through to typing)
//! live here, pure and tested. Reusable for any modeless count command.

/// What the key handler should do after feeding one key.
#[derive(Debug, Clone, PartialEq)]
pub enum CountAction {
    /// a digit was absorbed into the pending count — consume the key, no command.
    Absorbed(u32),
    /// a command key fired: run it with this count (>=1). Consume the key.
    Fire { cmd: char, count: u32 },
    /// not ours — reset any pending count and let the key pass through (do NOT
    /// consume), so typing/URL-bar/forms keep working.
    PassThrough,
}

/// The accumulator. `cmds` are the command chars that consume the pending count
/// and fire (case-sensitive, e.g. ['b','B']). `cap` bounds the count so a long
/// digit run can't overflow or produce absurd values.
#[derive(Debug, Clone)]
pub struct CountPrefix {
    pending: u32,
    cap: u32,
    cmds: Vec<char>,
}

impl CountPrefix {
    pub fn new(cmds: &[char], cap: u32) -> Self {
        CountPrefix { pending: 0, cap: cap.max(1), cmds: cmds.to_vec() }
    }

    /// Current pending count (0 = none typed yet).
    pub fn pending(&self) -> u32 { self.pending }

    /// Reset the pending count (e.g. on focus loss / escape).
    pub fn reset(&mut self) { self.pending = 0; }

    /// Feed one key character. Returns the action to take.
    /// - a digit '0'..'9' accumulates (but a LEADING '0' is not a count — vim
    ///   treats `0` as its own motion; here we just pass it through so it never
    ///   silently eats a zero),
    /// - a command char fires with max(1, pending) then resets,
    /// - anything else resets and passes through.
    pub fn feed(&mut self, ch: char) -> CountAction {
        if ch.is_ascii_digit() {
            let d = ch as u32 - '0' as u32;
            // a leading zero is not a count prefix — pass it through untouched.
            if d == 0 && self.pending == 0 {
                return CountAction::PassThrough;
            }
            self.pending = (self.pending.saturating_mul(10).saturating_add(d)).min(self.cap);
            return CountAction::Absorbed(self.pending);
        }
        if self.cmds.contains(&ch) {
            let count = self.pending.max(1);
            self.pending = 0;
            return CountAction::Fire { cmd: ch, count };
        }
        // not a digit, not a command → abandon any pending count, pass through.
        self.pending = 0;
        CountAction::PassThrough
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp() -> CountPrefix { CountPrefix::new(&['b', 'B'], 99) }

    #[test]
    fn bare_command_fires_count_one() {
        let mut c = cp();
        assert_eq!(c.feed('b'), CountAction::Fire { cmd: 'b', count: 1 });
    }

    #[test]
    fn single_digit_then_command() {
        let mut c = cp();
        assert_eq!(c.feed('3'), CountAction::Absorbed(3));
        assert_eq!(c.feed('b'), CountAction::Fire { cmd: 'b', count: 3 });
        // count cleared after firing
        assert_eq!(c.feed('B'), CountAction::Fire { cmd: 'B', count: 1 });
    }

    #[test]
    fn multi_digit_accumulates() {
        let mut c = cp();
        assert_eq!(c.feed('1'), CountAction::Absorbed(1));
        assert_eq!(c.feed('2'), CountAction::Absorbed(12));
        assert_eq!(c.feed('B'), CountAction::Fire { cmd: 'B', count: 12 });
    }

    #[test]
    fn count_is_capped() {
        let mut c = CountPrefix::new(&['b'], 50);
        c.feed('9'); c.feed('9'); c.feed('9'); // 999 → capped 50
        assert_eq!(c.pending(), 50);
        assert_eq!(c.feed('b'), CountAction::Fire { cmd: 'b', count: 50 });
    }

    #[test]
    fn leading_zero_passes_through() {
        let mut c = cp();
        assert_eq!(c.feed('0'), CountAction::PassThrough);
        assert_eq!(c.pending(), 0);
        // but a zero AFTER a nonzero digit is a real part of the count
        c.feed('1');
        assert_eq!(c.feed('0'), CountAction::Absorbed(10));
    }

    #[test]
    fn non_command_resets_and_passes() {
        let mut c = cp();
        c.feed('5');
        assert_eq!(c.feed('x'), CountAction::PassThrough);
        assert_eq!(c.pending(), 0);
        // a following 'b' is now a bare command (count 1), not 5
        assert_eq!(c.feed('b'), CountAction::Fire { cmd: 'b', count: 1 });
    }

    #[test]
    fn reset_clears_pending() {
        let mut c = cp();
        c.feed('7');
        c.reset();
        assert_eq!(c.pending(), 0);
        assert_eq!(c.feed('b'), CountAction::Fire { cmd: 'b', count: 1 });
    }

    #[test]
    fn unrelated_keys_never_consumed() {
        let mut c = cp();
        // typing "abc" must all pass through untouched (no count, no command)
        for ch in "ac".chars() {
            assert_eq!(c.feed(ch), CountAction::PassThrough);
        }
    }
}
