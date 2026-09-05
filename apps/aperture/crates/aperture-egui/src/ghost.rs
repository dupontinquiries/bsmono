//! ghost — the inline-suggestion state machine, egui-free.
//!
//! Three states and every transition is a method, so the whole lifecycle is
//! unit-tested here and the view layer only *draws*:
//!
//! ```text
//! Idle ──request──▶ Pending{gen} ──outcome(gen matches)──▶ Shown{text}
//!   ▲                   │  outcome stale/err → Idle           │
//!   └──── dismiss ──────┴──── edit that diverges ─────────────┘
//!                              (typing the ghost's own next chars instead
//!                               CONSUMES them and stays Shown)
//! ```
//!
//! The typing-through rule is the one that makes ghost text feel like an
//! instrument instead of a popup: if the suggestion is `intln!("hi");` and
//! you type `i`, the ghost doesn't flicker away and re-request — it eats the
//! `i` and keeps offering the rest.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ghost {
    Idle,
    Pending { generation: u64 },
    /// `generation` is what makes STREAMING actually work: a completion
    /// in progress sends several outcomes for the same request, and once
    /// the first one lands the ghost is already `Shown`, not `Pending`.
    /// Without tracking which generation is currently showing, `on_outcome`
    /// had no way to recognize "this is another delta for the SAME
    /// request" and silently dropped every partial after the first —
    /// including the final one — because its guard only ever accepted an
    /// outcome while still `Pending`. Found via an end-to-end test that
    /// watched the ghost text over a real stream and never saw it grow
    /// past the first delta.
    Shown { text: String, generation: u64 },
}

impl Ghost {
    pub fn is_shown(&self) -> bool {
        matches!(self, Ghost::Shown { .. })
    }
    pub fn is_pending(&self) -> bool {
        matches!(self, Ghost::Pending { .. })
    }

    pub fn request_sent(&mut self, generation: u64) {
        *self = Ghost::Pending { generation };
    }

    /// Feed an engine outcome. Accepts a success matching the buffer
    /// version we're *now* at, whether the ghost is currently `Pending`
    /// for this generation (the first delta, or a non-streaming answer)
    /// OR already `Shown` for this SAME generation (a later delta in the
    /// same stream) — anything else quietly returns to Idle (an error
    /// message is the caller's to log — the loop pane does).
    /// Returns the multipass MENU when one was produced: the blocks
    /// beyond the first, plus the model's prose. The caller stores them;
    /// the ghost stays a single string so every existing site — accept,
    /// accept-word, dismiss, the tests — keeps working unchanged.
    ///
    /// Returning rather than storing is deliberate. `Ghost` is a small
    /// value type that several paths pattern-match on, and widening its
    /// `Shown` variant to carry a menu would touch ten call sites to
    /// deliver something only one of them wants.
    pub fn on_outcome(
        &mut self,
        generation: u64,
        current_version: u64,
        result: &Result<String, String>,
        // The operator's binding order. Passed in rather than read from
        // a global so this stays testable with any map, and so the editor
        // owns the setting rather than the completion layer owning it.
        map: &aperture_complete::keymap::KeyMap,
    ) -> Option<(Vec<aperture_complete::multipass::Block>, Vec<String>)> {
        let expected = matches!(*self, Ghost::Pending { generation: g } if g == generation)
            || matches!(*self, Ghost::Shown { generation: g, .. } if g == generation);
        match result {
            Ok(text) if expected && generation == current_version && !text.is_empty() => {
                // MULTIPASS, rather than showing the reply verbatim.
                //
                // A raw reply mixes several kinds of thing: real code,
                // hallucinated constructs, an elision like "// ... rest
                // of the code remains the same" that DELETES a function
                // if accepted, a `fn main() { ... }` stub, and prose
                // explaining the code to someone already looking at it.
                // Inserting all of that costs more than typing it, which
                // is what "relies on manual cleanup afterward" meant.
                //
                // `plan` splits it into keyed blocks and separates the
                // prose. The FIRST insertable block becomes the ghost, so
                // Tab still means "take the obvious one" and nothing has
                // to change for the common case; the rest are offered on
                // home-row keys.
                let (blocks, prose) = aperture_complete::multipass::plan_with(text, map);
                match blocks.iter().find(|b| b.kind.insertable()) {
                    Some(first) => {
                        *self = Ghost::Shown { text: first.text.clone(), generation };
                    }
                    // Every block was an elision, a stub, or prose. There
                    // is nothing safe to insert, and showing the raw text
                    // anyway is exactly the behaviour being fixed.
                    None => *self = Ghost::Idle,
                }
                return Some((blocks, prose));
            }
            _ => {
                if expected {
                    *self = Ghost::Idle;
                }
            }
        }
        None
    }

    /// The user typed `typed`. Returns true when the ghost consumed it
    /// (i.e. the edit matched the suggestion's own next chars).
    pub fn on_text_inserted(&mut self, typed: &str) -> bool {
        if let Ghost::Shown { text, .. } = self {
            if let Some(rest) = text.strip_prefix(typed) {
                if rest.is_empty() {
                    *self = Ghost::Idle; // typed the whole thing yourself
                } else {
                    *text = rest.to_string();
                }
                return true;
            }
        }
        *self = Ghost::Idle;
        false
    }

    /// Any non-matching mutation, caret move, or Escape.
    pub fn dismiss(&mut self) {
        *self = Ghost::Idle;
    }

    /// Take the full suggestion for insertion.
    /// The suggestion text, if one is showing.
    ///
    /// Borrowed rather than taken: starting a typeout must not clear the
    /// ghost, because the ghost is what draws the remaining suggestion
    /// ahead of the caret while it is being typed out.
    pub fn text(&self) -> Option<&str> {
        match self {
            Ghost::Shown { text, .. } => Some(text),
            _ => None,
        }
    }

    /// Shrink the suggestion by text that has just been inserted.
    ///
    /// Without this the ghost would keep drawing the accepted text ahead
    /// of the caret, so a typed-out word would appear twice: once as
    /// real text and once as the suggestion of itself.
    ///
    /// A MISMATCH DISMISSES rather than trying to resync. If the inserted
    /// text is not the ghost's own prefix, the two have diverged — an
    /// edit landed in between — and a ghost that no longer describes what
    /// follows the caret is worse than none.
    pub fn consume(&mut self, typed: &str) {
        let Ghost::Shown { text, .. } = self else { return };
        match text.strip_prefix(typed) {
            Some(rest) if !rest.is_empty() => *text = rest.to_string(),
            Some(_) => *self = Ghost::Idle, // fully typed out
            None => *self = Ghost::Idle,
        }
    }

    pub fn accept_all(&mut self) -> Option<String> {
        match std::mem::replace(self, Ghost::Idle) {
            Ghost::Shown { text, .. } => Some(text),
            other => {
                *self = other; // put a Pending back; taking it would drop a request
                None
            }
        }
    }

    /// Take just the next word (leading spaces + run of word chars, or a
    /// single newline+indent) — the Ctrl+→ partial accept.
    pub fn accept_word(&mut self) -> Option<String> {
        let Ghost::Shown { text, generation } = self else { return None };
        let generation = *generation;
        let word = next_word(text);
        if word.is_empty() {
            return None;
        }
        let rest = text[word.len()..].to_string();
        let out = word.to_string();
        if rest.is_empty() {
            *self = Ghost::Idle;
        } else {
            *self = Ghost::Shown { text: rest, generation };
        }
        Some(out)
    }
}

fn next_word(s: &str) -> &str {
    let b = s.as_bytes();
    if b.first() == Some(&b'\n') {
        // newline + its indentation is one "word": accepting it lands you
        // at the start of the suggested next line, ready for the next tap.
        let mut i = 1;
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        return &s[..i];
    }
    let mut i = 0;
    while i < b.len() && b[i] == b' ' {
        i += 1;
    }
    let start_alpha = i;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_alphanumeric() || c == '_' {
            i += 1;
        } else {
            break;
        }
    }
    if i == start_alpha && i < b.len() && b[i] != b'\n' {
        i += 1; // a lone punctuation char counts as the next word
    }
    &s[..i]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_pending_to_shown_gated_by_generation() {
        let mut g = Ghost::Idle;
        g.request_sent(5);
        g.on_outcome(4, 5, &Ok("stale".into()), &aperture_complete::keymap::KeyMap::new());
        assert!(g.is_pending(), "stale outcome ignored, still waiting");
        g.on_outcome(5, 6, &Ok("late".into()), &aperture_complete::keymap::KeyMap::new());
        assert_eq!(g, Ghost::Idle, "right request, but the buffer moved on");
        g.request_sent(7);
        g.on_outcome(7, 7, &Ok("println!".into()), &aperture_complete::keymap::KeyMap::new());
        assert_eq!(g, Ghost::Shown { text: "println!".into(), generation: 7 });
    }

    #[test]
    fn a_later_delta_for_the_same_generation_updates_an_already_shown_ghost() {
        // THE bug: on_outcome's guard originally only accepted an outcome
        // while still Pending, so the first streamed delta landing turned
        // the ghost Shown — permanently locking out every later delta for
        // that same request, the final one included. Found by an
        // end-to-end test that watched real streamed ghost text and never
        // saw it grow past the first fragment.
        //
        // Deltas here avoid leading/trailing whitespace on purpose:
        // `multipass::make_block` trims each block's text, which is a
        // SEPARATE, minor, self-correcting quirk when applied to a
        // streaming partial (an intermediate delta like "fn " loses its
        // trailing space until the next delta arrives) — real, but not
        // what this test is isolating.
        let mut g = Ghost::Idle;
        g.request_sent(3);
        g.on_outcome(3, 3, &Ok("fn".into()), &aperture_complete::keymap::KeyMap::new());
        assert_eq!(g, Ghost::Shown { text: "fn".into(), generation: 3 });
        g.on_outcome(3, 3, &Ok("fn main".into()), &aperture_complete::keymap::KeyMap::new());
        assert_eq!(g, Ghost::Shown { text: "fn main".into(), generation: 3 },
            "a second delta for the SAME generation must still update the ghost, not be silently dropped");
        g.on_outcome(3, 3, &Ok("fn main(){}".into()), &aperture_complete::keymap::KeyMap::new());
        assert_eq!(g, Ghost::Shown { text: "fn main(){}".into(), generation: 3 });
    }

    #[test]
    fn a_delta_for_a_different_generation_while_shown_is_still_rejected() {
        // The fix must not become "accept anything while Shown" — an
        // outcome for a DIFFERENT (older or stale) generation arriving
        // while a newer one is already showing must still be ignored,
        // same as it always was for the Pending case.
        let mut g = Ghost::Idle;
        g.request_sent(2);
        g.on_outcome(2, 2, &Ok("current".into()), &aperture_complete::keymap::KeyMap::new());
        assert_eq!(g, Ghost::Shown { text: "current".into(), generation: 2 });
        // A straggler from an OLDER generation (1) arrives after 2 is
        // already showing — must not overwrite it.
        g.on_outcome(1, 2, &Ok("stale".into()), &aperture_complete::keymap::KeyMap::new());
        assert_eq!(g, Ghost::Shown { text: "current".into(), generation: 2 },
            "an outcome for a generation that doesn't match the one currently Shown must be a no-op, not overwrite it");
    }

    #[test]
    fn a_stale_delta_with_the_current_version_moved_on_is_still_rejected_while_shown() {
        let mut g = Ghost::Idle;
        g.request_sent(1);
        g.on_outcome(1, 1, &Ok("old".into()), &aperture_complete::keymap::KeyMap::new());
        assert_eq!(g, Ghost::Shown { text: "old".into(), generation: 1 });
        // A late straggler from the SAME generation arrives, but the
        // CALLER is now on generation 2 (current_version moved on).
        g.on_outcome(1, 2, &Ok("stale".into()), &aperture_complete::keymap::KeyMap::new());
        assert_eq!(g, Ghost::Idle, "generation matches self, but current_version moved on");
    }

    #[test]
    fn typing_through_consumes_and_divergence_dismisses() {
        let mut g = Ghost::Shown { text: "intln!(\"hi\")".into(), generation: 0 };
        assert!(g.on_text_inserted("in"));
        assert_eq!(g, Ghost::Shown { text: "tln!(\"hi\")".into(), generation: 0 });
        assert!(!g.on_text_inserted("x"), "divergent keystroke");
        assert_eq!(g, Ghost::Idle);
    }

    #[test]
    fn word_accept_walks_the_suggestion() {
        let mut g = Ghost::Shown { text: "let mut x = 1;\n    next()".into(), generation: 0 };
        assert_eq!(g.accept_word().unwrap(), "let");
        assert_eq!(g.accept_word().unwrap(), " mut");
        assert_eq!(g.accept_word().unwrap(), " x");
        assert_eq!(g.accept_word().unwrap(), " =");
        assert_eq!(g.accept_word().unwrap(), " 1");
        assert_eq!(g.accept_word().unwrap(), ";");
        assert_eq!(g.accept_word().unwrap(), "\n    ");
        assert_eq!(g.accept_word().unwrap(), "next");
        let mut all = Ghost::Shown { text: "()".into(), generation: 0 };
        assert_eq!(all.accept_all().unwrap(), "()");
        assert_eq!(all, Ghost::Idle);
    }

    #[test]
    fn accepting_a_word_mid_stream_is_not_reset_by_the_next_delta() {
        // A NEW interaction the generation fix makes possible for the
        // first time (streaming was frozen before, so nobody could
        // "accept mid-stream" and then have another delta arrive): if a
        // person hits Ctrl+-> and types out part of the suggestion WHILE
        // the server still has more coming, does the next delta correctly
        // replace only what's LEFT, or does it silently reintroduce text
        // that was already accepted by resetting to the server's own
        // (unaware-of-the-accept) accumulated string?
        let mut g = Ghost::Idle;
        g.request_sent(1);
        g.on_outcome(1, 1, &Ok("let x".into()), &aperture_complete::keymap::KeyMap::new());
        assert_eq!(g, Ghost::Shown { text: "let x".into(), generation: 1 });
        let taken = g.accept_word().unwrap();
        assert_eq!(taken, "let");
        assert_eq!(g, Ghost::Shown { text: " x".into(), generation: 1 }, "remainder after accepting 'let'");
        // The next streamed delta arrives — the server's own accumulated
        // text so far, which has NO IDEA "let" was already typed out.
        g.on_outcome(1, 1, &Ok("let x = 1".into()), &aperture_complete::keymap::KeyMap::new());
        assert_eq!(
            g,
            Ghost::Shown { text: "let x = 1".into(), generation: 1 },
            "documents the CURRENT behavior: the next delta replaces the ghost with the \
             server's full accumulated text, which does not subtract what accept_word already \
             took. If this test starts failing because someone 'fixed' on_outcome to preserve \
             the post-accept remainder, that is the improvement — this exists to make the \
             tradeoff visible and require an explicit decision, not to bless it as correct."
        );
    }
}
